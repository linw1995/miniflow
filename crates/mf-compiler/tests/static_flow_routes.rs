//! A fixed capability map specializes dispatch into a gate and static Calls.
extern crate mfn_core as _;

use mf_compiler::{
    NodeRegistry, WorkflowDefinition, compile_definition, describe_compiled, instantiate_compiled,
};
use mf_runtime::{
    ExecutionContext, Inputs, NodeExecutionError, NodeFactory, NodeMetadata, NodePortContract,
    NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode, TaskNode, ValueRef,
    ValueType, WorkflowArguments,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

static FIRST: AtomicUsize = AtomicUsize::new(0);
static SECOND: AtomicUsize = AtomicUsize::new(0);

struct Effect {
    first: bool,
}
impl NodePortContract for Effect {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        }
    }
}
impl TaskNode for Effect {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if self.first {
            FIRST.fetch_add(1, Ordering::SeqCst);
        } else {
            SECOND.fetch_add(1, Ordering::SeqCst);
        }
        assert!(context.invocation_path().last().unwrap().is_call());
        Ok(Outputs::from([("value".into(), inputs["value"].clone())]).into())
    }
}
inventory::submit! {
    NodeRegistration { kind: "test.static-route-effect", factory: NodeFactory::Plain(|config| {
        PreparedNode::new(Effect { first: config["first"] == true }, NodeMetadata::default())
    }) }
}

// An opaque filter must retain the full incoming provenance, not just the
// runtime-selected keys. Substituting a handle cannot replace a static body.
struct Filter;
impl NodePortContract for Filter {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortSpec::new("tools", ValueType::Map(Box::new(ValueType::Flow)), true),
                PortSpec::new("enabled", ValueType::Int64, true),
            ],
            outputs: vec![PortSpec::new(
                "value",
                ValueType::Map(Box::new(ValueType::Flow)),
                true,
            )],
        }
    }
}
impl TaskNode for Filter {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let tools = &inputs["tools"];
        let entries = match inputs["enabled"].as_i64().unwrap() {
            0 => vec![],
            1 => vec![("first".into(), tools["first"].clone())],
            2 => vec![("second".into(), tools["second"].clone())],
            4 => vec![("first".into(), tools["second"].clone())],
            5 => vec![("new".into(), tools["first"].clone())],
            _ => tools
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        };
        Ok(Outputs::from([("value".into(), ValueRef::object(entries))]).into())
    }
}
inventory::submit! {
    NodeRegistration { kind: "test.static-route-filter", factory: NodeFactory::Plain(|_| PreparedNode::new(Filter, NodeMetadata::default())) }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.static-route-source",
        factory: NodeFactory::WithFlowBindings {
            factory: &NodeFactory::Subgraph {
                scope: None,
                flow_parameters: |config| BTreeMap::from([("body".into(), config["body"].clone())]),
                plain: |_| mf_runtime::NodeInvalidSubgraphSnafu { message:"a prepared body is required" }.fail(),
                prepared: |id, config, mut bodies| NodeRegistry::from_inventory().unwrap().get("builtin.constant").unwrap().instantiate_subgraphs(
                    id, json!({"type":"flow", "value":config["body"]}), BTreeMap::from([("value".into(), bodies.remove("body").unwrap())]),
                ),
            },
            bindings: |_| mf_runtime::FlowBindings {
                outputs: BTreeMap::from([("value".into(), mf_runtime::FlowBindingSource::Child("body".into()))]),
                ..Default::default()
            },
        },
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "test.host-static-route-source",
        factory: NodeFactory::WithFlowBindings {
            factory: &NodeFactory::Subgraph {
                scope: Some(mf_runtime::SubgraphScope::Host),
                flow_parameters: |config| BTreeMap::from([("body".into(),config["body"].clone())]),
                plain: |_| mf_runtime::NodeInvalidSubgraphSnafu { message:"a prepared body is required" }.fail(),
                prepared: |id, config, bodies| NodeRegistry::from_inventory().unwrap().get("test.static-route-source").unwrap().instantiate_subgraphs(id,config,bodies),
            },
            bindings: |config| NodeRegistry::from_inventory().unwrap().get("test.static-route-source").unwrap().factory.flow_bindings(config),
        },
    }
}

#[test]
fn host_owned_static_calls_do_not_require_unlinked_body_providers() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let body = WorkflowDefinition::from_json(
        &json!({"version":"2026-11-13",
        "nodes":[{"id":"unlinked","kind":"test.unlinked-provider"}],
        "outputs":[{"name":"value","node":"unlinked","port":"value"}]})
        .to_string(),
    )
    .unwrap();
    let schema = serde_json::from_value(
        json!({"inputs":{"unlinked":{"value":{"type":"int","required":true}}},"stdin":{}}),
    )
    .unwrap();
    registry
        .register_flow(
            "external",
            body.clone(),
            mf_runtime::PreparedSubgraph::new(
                vec![],
                vec![PortSpec::new("value", ValueType::Int64, true)],
                |context| {
                    Ok(Outputs::from([(
                        "value".into(),
                        context.workflow_arguments().0["unlinked.value"].clone(),
                    )]))
                },
            )
            .with_input_schema(schema),
        )
        .unwrap();
    let mut source = definition();
    let first = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "first")
        .unwrap();
    first.kind = "test.host-static-route-source".into();
    first.config = json!({"flow":"external","body":body});
    let plan = compile_definition(&source, &registry).unwrap();
    assert_eq!(
        plan.graph.children[&"execute:route:first".into()]["body"]
            .relations
            .host
            .as_deref(),
        Some("external")
    );
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"]["outputs"]["value"],
        json!(10)
    );
}

#[test]
fn one_callee_source_is_specialized_independently_for_each_call_site() {
    let callee = json!({"version":"2026-11-13","nodes":[
        {"id":"params","kind":"builtin.parameters","config":{"parameters":{
            "tools":{"type":{"map":"flow"},"secret":false},
            "route":{"type":"string","secret":false},
            "arguments":{"type":"object","secret":false}
        }}},
        {"id":"execute","kind":"workflow.call","config":{"dispatch":true},"inputs":{
            "flows":edge("params","tools"),"route":edge("params","route"),"arguments":edge("params","arguments")
        }}],"outputs":[{"name":"result","node":"execute","port":"result"}]});
    let mut value = serde_json::to_value(definition()).unwrap();
    value["nodes"].as_array_mut().unwrap().retain(|node| {
        matches!(
            node["id"].as_str(),
            Some("first" | "second" | "params" | "arguments")
        )
    });
    for route in ["first", "second"] {
        value["nodes"].as_array_mut().unwrap().extend([
            json!({"id":format!("{route}_map"),"kind":"builtin.object","config":{"fields":{route:"flow"}},"inputs":{route:edge(route,"value")}}),
            json!({"id":format!("{route}_route"),"kind":"builtin.constant","config":{"value":route}}),
            json!({"id":format!("{route}_call"),"kind":"workflow.call","config":{"flow":"callee","body":callee},"inputs":{
                "params.tools":edge(&format!("{route}_map"),"value"),"params.route":edge(&format!("{route}_route"),"value"),"params.arguments":edge("arguments","value")
            }})
        ]);
    }
    value["outputs"] = json!([
        {"name":"first","node":"first_call","port":"result"},
        {"name":"second","node":"second_call","port":"result"}
    ]);
    let source = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&source, &registry).unwrap();
    for route in ["first", "second"] {
        let child = &plan.graph.children[&format!("{route}_call").into()]["body"];
        assert_eq!(
            child
                .definition
                .nodes
                .iter()
                .find(|node| node.id.as_str() == "execute")
                .unwrap()
                .config["routes"],
            json!([route])
        );
        assert_eq!(child.children.len(), 1);
    }
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(output["first"]["outputs"]["value"], json!(10));
    assert_eq!(output["second"]["outputs"]["value"], json!(10));
}

fn edge(node: &str, port: &str) -> Value {
    json!({"from":{"node":node,"port":port}})
}
fn effect(first: bool) -> Value {
    json!({"version":"2026-11-13", "nodes":[{"id":"effect", "kind":"test.static-route-effect", "config":{"first":first}}],
        "outputs":[{"name":"value", "node":"effect", "port":"value"}]})
}
fn definition() -> WorkflowDefinition {
    WorkflowDefinition::from_json(&json!({"version":"2026-11-13", "nodes":[
        {"id":"first", "kind":"builtin.constant", "config":{"type":"flow", "value":effect(true)}},
        {"id":"second", "kind":"builtin.constant", "config":{"type":"flow", "value":effect(false)}},
        {"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
            "enabled":{"type":"int", "default":3, "secret":false},
            "route":{"type":"string", "default":"first", "secret":false},
            "value":{"type":"int", "default":10, "secret":false}
        }}},
        {"id":"tools", "kind":"builtin.object", "config":{"fields":{"first":"flow", "second":"flow"}},
            "inputs":{"first":edge("first","value"), "second":edge("second","value")}},
        {"id":"filter", "kind":"test.static-route-filter", "inputs":{"tools":edge("tools","value"), "enabled":edge("params","enabled")}},
        {"id":"arguments", "kind":"builtin.object", "config":{"fields":{"value":"int"}}, "inputs":{"value":edge("params","value")}},
        {"id":"execute", "kind":"workflow.call", "config":{"dispatch":true},
            "inputs":{"route":edge("params","route"), "flows":edge("filter","value"), "arguments":edge("arguments","value")}}
    ], "outputs":[{"name":"result", "node":"execute", "port":"result"}]}).to_string()).unwrap()
}
fn arguments(enabled: i64, route: &str, value: i64) -> WorkflowArguments {
    WorkflowArguments::try_from(
        json!({"params.enabled":enabled,"params.route":route,"params.value":value}),
    )
    .unwrap()
}
fn assert_static_calls(graph: &mf_runtime::CompiledGraph) {
    assert!(!graph.children.contains_key(&"execute".into()));
    let router = graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "execute")
        .unwrap();
    assert_eq!(router.kind, "builtin.router");
    assert_eq!(router.config, json!({"routes":["first","second"]}));
    for route in ["first", "second"] {
        let call_id = format!("execute:route:{route}");
        let call = graph
            .definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == call_id)
            .unwrap();
        assert_eq!(call.kind, "workflow.call");
        assert_eq!(call.config["flow"], route);
        assert_eq!(graph.children[&call_id.into()].len(), 1);
    }
}

#[test]
fn filtering_removing_and_readding_routes_keeps_the_same_static_calls_and_description() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let source = definition();
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_calls(&plan.graph);
    let description = describe_compiled(&plan).unwrap();
    for route in ["first", "second"] {
        assert_eq!(
            description
                .body_paths_for_node(&[], &format!("execute:route:{route}"))
                .len(),
            1
        );
    }
    let bytes = plan.manifest(&registry).unwrap().to_bytes().unwrap();
    assert_eq!(
        mf_runtime::WorkflowManifest::from_bytes(&bytes)
            .unwrap()
            .description()
            .unwrap(),
        description
    );
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (enabled, route) in [(1, "first"), (2, "second"), (3, "first"), (1, "first")] {
        assert_eq!(
            flow.execute_with_inputs(arguments(enabled, route, 17))
                .unwrap()["result"]["outputs"]["value"],
            json!(17)
        );
    }
    for (enabled, route) in [(0, "first"), (1, "second"), (2, "first"), (3, "missing")] {
        assert!(
            flow.execute_with_inputs(arguments(enabled, route, 0))
                .unwrap()["result"]["error"]
                .as_str()
                .unwrap()
                .contains("disabled")
        );
    }
    assert_eq!(FIRST.load(Ordering::SeqCst), 3);
    assert_eq!(SECOND.load(Ordering::SeqCst), 1);
    assert_eq!(describe_compiled(&plan).unwrap(), description);
}

#[test]
fn runtime_handles_cannot_replace_static_targets_or_add_routes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let flow = instantiate_compiled(
        &compile_definition(&definition(), &registry).unwrap(),
        &registry,
    )
    .unwrap();
    assert_eq!(
        flow.execute_with_inputs(arguments(4, "first", 12)).unwrap()["result"]["outputs"]["value"],
        json!(12)
    );
    assert_eq!(FIRST.load(Ordering::SeqCst), 1);
    assert_eq!(SECOND.load(Ordering::SeqCst), 0);
    let error = flow
        .execute_with_inputs(arguments(5, "new", 0))
        .unwrap_err();
    assert!(error.to_string().contains("enables"), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn every_static_body_is_checked_even_when_its_route_starts_disabled() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut body = effect(false);
    body["outputs"][0]["port"] = json!("missing");
    let source = WorkflowDefinition::from_json(&json!({"version":"2026-11-13","nodes":[
        {"id":"route","kind":"builtin.constant","config":{"value":"second"}},
        {"id":"choose","kind":"builtin.router","config":{"routes":["second"],"enables":[]},"inputs":{"route":edge("route","value")}},
        {"id":"call","kind":"workflow.call","config":{"flow":"tool","body":body}}
    ],"edges":[{"from_node":"choose","from_output":"second","to_node":"call"}]}).to_string()).unwrap();
    assert!(compile_definition(&source, &registry).is_err());
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
    assert_eq!(SECOND.load(Ordering::SeqCst), 0);
}

#[test]
fn static_calls_validate_runtime_arguments_before_business_effects() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition();
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "arguments")
        .unwrap()
        .config["fields"]["value"] = json!("string");
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "params")
        .unwrap()
        .config["parameters"]["value"] =
        json!({"type":"string", "default":"wrong", "secret":false});
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_calls(&plan.graph);
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert!(output["result"]["error"].as_str().unwrap().contains("int"));
    assert!(output["result"].is_secret());
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
}

#[test]
fn provider_metadata_authorizes_provenance_without_a_builtin_producer_name() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition();
    for node in &mut source.nodes {
        if matches!(node.id.as_str(), "first" | "second") {
            node.kind = "test.static-route-source".into();
            node.config = json!({"body":node.config["value"]});
        }
    }
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_calls(&plan.graph);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"]["outputs"]["value"],
        json!(10)
    );
}

#[test]
fn startup_nodes_forward_provenance_across_a_configured_call() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition();
    let filter = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "filter")
        .unwrap();
    filter.kind = "workflow.call".into();
    filter.config = json!({"flow":"filter", "body":{"version":"2026-11-13",
        "nodes":[{"id":"filter", "kind":"test.static-route-filter"}],
        "outputs":[{"name":"value", "node":"filter", "port":"value"}]}});
    for edge in &mut source.edges {
        if edge.to_node.as_str() == "filter" {
            edge.to_input = format!("filter.{}", edge.to_input);
        }
    }
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_calls(&plan.graph);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments(2, "second", 19))
            .unwrap()["result"]["outputs"]["value"],
        json!(19)
    );
}

#[test]
fn workflow_state_assignments_preserve_the_static_superset() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(definition()).unwrap();
    value["variables"] = json!([{"name":"available","type":{"map":"flow"},"initial":{}}]);
    value["nodes"].as_array_mut().unwrap().push(json!({"id":"write","kind":"workflow.assign","config":{"variable":"available"},"inputs":{"value":edge("filter","value")}}));
    // Serialized modern definitions carry data wiring on nodes, not in edges.
    let execute = value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "execute")
        .unwrap();
    execute["inputs"]["flows"] = edge("%root", "available");
    value["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"write","from_output":"done","to_node":"execute"}));
    let source = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_calls(&plan.graph);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments(2, "second", 12))
            .unwrap()["result"]["outputs"]["value"],
        json!(12)
    );
}

#[test]
fn nested_dispatches_are_specialized_at_their_actual_call_sites() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition();
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "first")
        .unwrap()
        .config["value"] = serde_json::to_value(definition()).unwrap();
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "arguments")
        .unwrap()
        .config["fields"] = json!({});
    source
        .edges
        .retain(|edge| edge.to_node.as_str() != "arguments");
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_calls(&plan.graph);
    assert_static_calls(&plan.graph.children[&"execute:route:first".into()]["body"]);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"]["outputs"]["result"]["outputs"]["value"],
        json!(10)
    );
}

fn state_flow(target: &str) -> WorkflowDefinition {
    WorkflowDefinition::from_json(&json!({"version":"2026-11-13", "nodes":[
        {"id":"params","kind":"builtin.parameters","config":{"parameters":{
            "enables":{"type":{"list":"string"},"secret":false},
            "route":{"type":"string","default":"first","secret":false}
        }}},
        {"id":"choose","kind":"builtin.router","config":{"routes":["first","second"],"enables":["first"]},"inputs":{"route":edge("params","route")}},
        {"id":"write","kind":"workflow.assign","config":{"variable":target},"inputs":{"value":edge("params","enables")}},
        {"id":"read","kind":"builtin.identity","inputs":{"input":edge("%choose","enables")}}
    ],"edges":[{"from_node":"write","from_output":"done","to_node":"choose"},{"from_node":"write","from_output":"done","to_node":"read"}],
    "outputs":[{"name":"routes","node":"%choose","port":"routes"},{"name":"enables","node":"read","port":"value"},
        {"name":"first","node":"choose","port":"first","optional":true},
        {"name":"second","node":"choose","port":"second","optional":true},
        {"name":"else","node":"choose","port":"else","optional":true}]}).to_string()).unwrap()
}

#[test]
fn assign_replaces_only_enables_and_each_invocation_keeps_independent_state() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&state_flow("%choose.enables"), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (enables, route, selected) in [
        (json!([]), "first", "else"),
        (json!(["second"]), "first", "else"),
        (json!(["second"]), "second", "second"),
        (json!(["first", "second"]), "first", "first"),
    ] {
        let output = flow
            .execute_with_inputs(
                WorkflowArguments::try_from(json!({"params.enables":enables,"params.route":route}))
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(output["routes"], json!(["first", "second"]));
        assert_eq!(output["enables"], enables);
        assert_eq!(output[selected], json!(true));
        assert_eq!(output.len(), 3);
    }
    let router = plan
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "choose")
        .unwrap();
    assert_eq!(
        router.config,
        json!({"routes":["first","second"],"enables":["first"]})
    );
}

#[test]
fn node_state_rejects_readonly_unknown_duplicate_and_invalid_writes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for target in [
        "%choose.routes",
        "%choose.config.routes",
        "%missing.enables",
    ] {
        assert!(compile_definition(&state_flow(target), &registry).is_err());
    }
    let plan = compile_definition(&state_flow("%choose.enables"), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for enables in [
        json!(["missing"]),
        json!(["first", "first"]),
        json!([1]),
        json!({}),
    ] {
        assert!(
            flow.execute_with_inputs(
                WorkflowArguments::try_from(json!({"params.enables":enables})).unwrap()
            )
            .is_err()
        );
    }
}
