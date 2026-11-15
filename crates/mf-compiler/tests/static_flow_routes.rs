//! Authored routers own static route bodies without a compiler topology rewrite.
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
    collections::{BTreeMap, BTreeSet},
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
    NodeRegistration { kind:"test.static-route-effect", factory:NodeFactory::Plain(|config| PreparedNode::new(Effect { first:config["first"] == true },NodeMetadata::default())) }
}
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
    NodeRegistration { kind:"test.static-route-filter", factory:NodeFactory::Plain(|_| PreparedNode::new(Filter,NodeMetadata::default())) }
}
struct StaticKeys;
impl NodePortContract for StaticKeys {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("object", ValueType::Object, true)],
            outputs: vec![PortSpec::new(
                "keys",
                ValueType::List(Box::new(ValueType::String)),
                true,
            )],
        }
    }
}
impl TaskNode for StaticKeys {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let object = &inputs["object"];
        Ok(Outputs::from([(
            "keys".into(),
            ValueRef::array(
                object
                    .as_object()
                    .expect("validated object")
                    .keys()
                    .map(|key| ValueRef::from(key.as_ref())),
            ),
        )])
        .into())
    }
}
inventory::submit! {
    NodeRegistration { kind:"test.static-route-keys", factory:NodeFactory::Plain(|_| PreparedNode::new(StaticKeys,NodeMetadata::default())) }
}
inventory::submit! {
    NodeRegistration { kind:"test.static-route-source", factory:NodeFactory::WithFlowBindings {
        factory: &NodeFactory::Subgraph {
            scope:None, flow_parameters:|config| BTreeMap::from([("body".into(),config["body"].clone())]),
            plain:|_| mf_runtime::NodeInvalidSubgraphSnafu { message:"a prepared body is required" }.fail(),
            prepared:|id,config,mut bodies| NodeRegistry::from_inventory().unwrap().get("builtin.constant").unwrap().instantiate_subgraphs(
                id,json!({"type":"flow","value":config["body"]}),BTreeMap::from([("value".into(),bodies.remove("body").unwrap())])),
        },
        bindings:|_| mf_runtime::FlowBindings { outputs:BTreeMap::from([("value".into(),mf_runtime::FlowBindingSource::Child("body".into()))]), ..Default::default() },
    } }
}
inventory::submit! {
    NodeRegistration { kind:"test.host-static-route-source", factory:NodeFactory::WithFlowBindings {
        factory: &NodeFactory::Subgraph {
            scope:Some(mf_runtime::SubgraphScope::Host), flow_parameters:|config| BTreeMap::from([("body".into(),config["body"].clone())]),
            plain:|_| mf_runtime::NodeInvalidSubgraphSnafu { message:"a prepared body is required" }.fail(),
            prepared:|id,config,bodies| NodeRegistry::from_inventory().unwrap().get("test.static-route-source").unwrap().instantiate_subgraphs(id,config,bodies),
        },
        bindings:|config| NodeRegistry::from_inventory().unwrap().get("test.static-route-source").unwrap().factory.flow_bindings(config),
    } }
}
fn edge(node: &str, port: &str) -> Value {
    json!({"from":{"node":node,"port":port}})
}
fn effect(first: bool) -> Value {
    json!({"version":"2026-11-13","nodes":[{"id":"effect","kind":"test.static-route-effect","config":{"first":first}}],
        "outputs":[{"name":"value","node":"effect","port":"value"}]})
}
fn definition() -> WorkflowDefinition {
    WorkflowDefinition::from_json(&json!({"version":"2026-11-13","nodes":[
        {"id":"first","kind":"builtin.constant","config":{"type":"flow","value":effect(true)}},
        {"id":"second","kind":"builtin.constant","config":{"type":"flow","value":effect(false)}},
        {"id":"params","kind":"builtin.parameters","config":{"parameters":{
            "enabled":{"type":"int","default":3,"secret":false},"route":{"type":"string","default":"first","secret":false},"value":{"type":"int","default":10,"secret":false}
        }}},
        {"id":"tools","kind":"builtin.object","config":{"fields":{"first":"flow","second":"flow"}},"inputs":{"first":edge("first","value"),"second":edge("second","value")}},
        {"id":"filter","kind":"test.static-route-filter","inputs":{"tools":edge("tools","value"),"enabled":edge("params","enabled")}},
        {"id":"arguments","kind":"builtin.object","config":{"fields":{"value":"int"}},"inputs":{"value":edge("params","value")}},
        {"id":"enabled_routes","kind":"test.static-route-keys","inputs":{"object":edge("filter","value")}},
        {"id":"set_enables","kind":"workflow.assign","config":{"variable":"%execute.enables"},"inputs":{"value":edge("enabled_routes","keys")}},
        {"id":"execute","kind":"builtin.router","config":{"routes":[]},"inputs":{"route":edge("params","route"),"flows":edge("filter","value"),"arguments":edge("arguments","value")}}
    ],"edges":[{"from_node":"set_enables","from_output":"done","to_node":"execute"}],
    "outputs":[{"name":"result","node":"execute","port":"result"}]}).to_string()).unwrap()
}
fn arguments(enabled: i64, route: &str, value: i64) -> WorkflowArguments {
    WorkflowArguments::try_from(
        json!({"params.enabled":enabled,"params.route":route,"params.value":value}),
    )
    .unwrap()
}
fn assert_static_routes(graph: &mf_runtime::CompiledGraph) {
    let node = graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "execute")
        .unwrap();
    assert_eq!(node.kind, "builtin.router");
    assert_eq!(node.config["routes"], json!(["first", "second"]));
    assert_eq!(
        graph.children[&node.id]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        graph.children[&node.id]
            .values()
            .all(|child| child.relations.static_binding)
    );
}

#[test]
fn compiler_binds_router_children_without_rewriting_authored_nodes_or_data_edges() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let source = definition();
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_routes(&plan.graph);
    let authored: BTreeSet<_> = source.nodes.iter().map(|node| node.id.clone()).collect();
    let compiled: BTreeSet<_> = plan
        .graph
        .definition
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    assert_eq!(compiled, authored);
    assert!(
        !plan
            .graph
            .definition
            .nodes
            .iter()
            .any(|node| node.kind == "workflow.call")
    );
    assert_eq!(plan.graph.definition.edges.len(), source.edges.len());
    for edge in &source.edges {
        assert!(plan.graph.definition.edges.contains(edge));
    }
    assert_eq!(plan.graph.definition.outputs, source.outputs);
    let description = describe_compiled(&plan).unwrap();
    assert_eq!(description.body_paths_for_node(&[], "execute").len(), 2);
    let manifest = plan.manifest(&registry).unwrap();
    let restored = mf_runtime::WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(restored.description().unwrap(), description);
    assert_eq!(
        restored
            .graph
            .definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "execute")
            .unwrap()
            .config,
        json!({"routes":["first","second"]})
    );
}

#[test]
fn freezing_preserves_initial_enable_validation_and_rejects_legacy_fields() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for extra in [
        json!({"enables":["missing"]}),
        json!({"enables":["first","first"]}),
        json!({"mode":"execute"}),
        json!({"flow_field":"run"}),
        json!({"flow-route:first":{"path":"must-not-read.json"}}),
    ] {
        let mut source = definition();
        let router = source
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "execute")
            .unwrap();
        router
            .config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let error = compile_definition(&source, &registry).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        assert!(!error.to_string().contains("could not read"), "{error}");
        if extra.get("enables").is_some() {
            assert!(error.to_string().contains("enables"), "{error}");
        }
    }
}

#[test]
fn saved_router_bindings_cannot_omit_or_add_declared_children() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let original = compile_definition(&definition(), &registry).unwrap();
    for add in [false, true] {
        let mut plan = original.clone();
        let children = plan.graph.children.get_mut(&"execute".into()).unwrap();
        if add {
            children.insert("third".into(), children["first"].clone());
        } else {
            children.remove("first");
        }
        assert!(instantiate_compiled(&plan, &registry).is_err());
    }
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
    assert_eq!(SECOND.load(Ordering::SeqCst), 0);
}

#[test]
fn removing_and_readding_enabled_routes_keeps_the_same_static_children() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let before = describe_compiled(&plan).unwrap();
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
    assert_eq!(describe_compiled(&plan).unwrap(), before);
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
    assert!(
        flow.execute_with_inputs(arguments(5, "new", 0))
            .unwrap_err()
            .to_string()
            .contains("enables")
    );
}
#[test]
fn route_names_are_not_config_fields_or_generated_call_ids() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (first, second) in [
        ("result", "enables"),
        ("routes", "arguments"),
        ("mode", "flow_field"),
        ("set_enables", "route:first"),
    ] {
        let mut source = definition();
        source
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "tools")
            .unwrap()
            .config["fields"] = json!({first:"flow",second:"flow"});
        source
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "params")
            .unwrap()
            .config["parameters"]["route"]["default"] = json!(first);
        for edge in &mut source.edges {
            if edge.to_node.as_str() == "tools" {
                edge.to_input = if edge.to_input == "first" {
                    first.into()
                } else {
                    second.into()
                };
            }
        }
        let plan = compile_definition(&source, &registry).unwrap();
        assert!(plan.graph.children[&"execute".into()].contains_key(first));
        assert_eq!(
            instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()["result"]["outputs"]["value"],
            json!(10)
        );
    }
}
#[test]
fn disabled_branches_are_still_checked_and_invalid_runtime_arguments_have_no_effects() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition();
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "execute")
        .unwrap()
        .config["enables"] = json!([]);
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "second")
        .unwrap()
        .config["value"]["outputs"][0]["port"] = json!("missing");
    assert!(compile_definition(&source, &registry).is_err());
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
        .config["parameters"]["value"] = json!({"type":"string","default":"wrong","secret":false});
    let plan = compile_definition(&source, &registry).unwrap();
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert!(output["result"]["error"].as_str().unwrap().contains("int"));
    assert!(output["result"].is_secret());
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
    assert_eq!(SECOND.load(Ordering::SeqCst), 0);
}
#[test]
fn provenance_is_provider_declared_and_forwards_across_ordinary_calls() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition();
    for node in &mut source.nodes {
        if matches!(node.id.as_str(), "first" | "second") {
            node.kind = "test.static-route-source".into();
            node.config = json!({"body":node.config["value"]});
        }
    }
    let filter = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "filter")
        .unwrap();
    filter.kind = "workflow.call".into();
    filter.config = json!({"flow":"filter","body":{"version":"2026-11-13","nodes":[{"id":"filter","kind":"test.static-route-filter"}],"outputs":[{"name":"value","node":"filter","port":"value"}]}});
    for edge in &mut source.edges {
        if edge.to_node.as_str() == "filter" {
            edge.to_input = format!("filter.{}", edge.to_input);
        }
    }
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_routes(&plan.graph);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments(2, "second", 19))
            .unwrap()["result"]["outputs"]["value"],
        json!(19)
    );
}
#[test]
fn workflow_state_assignments_keep_provenance_and_explicit_reader_prerequisites() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(definition()).unwrap();
    value["variables"] = json!([{"name":"available","type":{"map":"flow"},"initial":{}}]);
    value["nodes"].as_array_mut().unwrap().push(json!({"id":"write","kind":"workflow.assign","config":{"variable":"available"},"inputs":{"value":edge("filter","value")}}));
    for node in value["nodes"].as_array_mut().unwrap() {
        if node["id"] == "execute" {
            node["inputs"]["flows"] = edge("%root", "available");
        }
        if node["id"] == "enabled_routes" {
            node["inputs"]["object"] = edge("%root", "available");
        }
    }
    value["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"write","from_output":"done","to_node":"enabled_routes"}));
    let source = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let plan = compile_definition(&source, &registry).unwrap();
    assert_static_routes(&plan.graph);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute_with_inputs(arguments(2, "second", 12))
            .unwrap()["result"]["outputs"]["value"],
        json!(12)
    );
}
#[test]
fn nested_router_children_use_the_normal_call_boundary_at_each_binding() {
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
    assert_static_routes(&plan.graph.children[&"execute".into()]["first"]);
    assert_eq!(
        instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap()["result"]["outputs"]["result"]["outputs"]["value"],
        json!(10)
    );
}
#[test]
fn host_owned_branches_do_not_require_unlinked_body_providers() {
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let body = WorkflowDefinition::from_json(&json!({"version":"2026-11-13","nodes":[{"id":"unlinked","kind":"test.unlinked-provider"}],"outputs":[{"name":"value","node":"unlinked","port":"value"}]}).to_string()).unwrap();
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
        plan.graph.children[&"execute".into()]["first"]
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
fn same_callee_source_binds_different_supersets_at_distinct_call_sites() {
    let callee = json!({"version":"2026-11-13","nodes":[
        {"id":"params","kind":"builtin.parameters","config":{"parameters":{"tools":{"type":{"map":"flow"},"secret":false},"route":{"type":"string","secret":false},"arguments":{"type":"object","secret":false}}}},
        {"id":"execute","kind":"builtin.router","config":{"routes":[]},"inputs":{"flows":edge("params","tools"),"route":edge("params","route"),"arguments":edge("params","arguments")}}
    ],"outputs":[{"name":"result","node":"execute","port":"result"}]});
    let mut value = serde_json::to_value(definition()).unwrap();
    value["nodes"].as_array_mut().unwrap().retain(|node| {
        matches!(
            node["id"].as_str(),
            Some("first" | "second" | "params" | "arguments")
        )
    });
    value["edges"] = json!([]);
    for route in ["first", "second"] {
        value["nodes"].as_array_mut().unwrap().extend([
            json!({"id":format!("{route}_map"),"kind":"builtin.object","config":{"fields":{route:"flow"}},"inputs":{route:edge(route,"value")}}),
            json!({"id":format!("{route}_name"),"kind":"builtin.constant","config":{"value":route}}),
            json!({"id":format!("{route}_call"),"kind":"workflow.call","config":{"flow":"callee","body":callee},"inputs":{"params.tools":edge(&format!("{route}_map"),"value"),"params.route":edge(&format!("{route}_name"),"value"),"params.arguments":edge("arguments","value")}})
        ]);
    }
    value["outputs"] = json!([{"name":"first","node":"first_call","port":"result"},{"name":"second","node":"second_call","port":"result"}]);
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(
        &WorkflowDefinition::from_json(&value.to_string()).unwrap(),
        &registry,
    )
    .unwrap();
    for route in ["first", "second"] {
        assert_eq!(
            plan.graph.children[&format!("{route}_call").into()]["body"].children
                [&"execute".into()]
                .keys()
                .collect::<Vec<_>>(),
            [route]
        );
    }
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(output["first"]["outputs"]["value"], json!(10));
    assert_eq!(output["second"]["outputs"]["value"], json!(10));
}
fn state_flow(target: &str) -> WorkflowDefinition {
    let mut value = serde_json::to_value(definition()).unwrap();
    let nodes = value["nodes"].as_array_mut().unwrap();
    nodes.retain(|node| {
        !matches!(
            node["id"].as_str(),
            Some("filter" | "enabled_routes" | "set_enables")
        )
    });
    nodes
        .iter_mut()
        .find(|node| node["id"] == "params")
        .unwrap()["config"]["parameters"]["enables"] =
        json!({"type":{"list":"string"},"secret":false});
    let router = nodes
        .iter_mut()
        .find(|node| node["id"] == "execute")
        .unwrap();
    router["config"] = json!({"routes":["first","second"],"enables":["first"]});
    router["inputs"]["flows"] = edge("tools", "value");
    nodes.push(json!({
        "id":"write","kind":"workflow.assign","config":{"variable":target},
        "inputs":{"value":edge("params","enables")}
    }));
    value["edges"] = json!([{"from_node":"write","from_output":"done","to_node":"execute"}]);
    value["outputs"] = json!([{"name":"result","node":"execute","port":"result"}]);
    WorkflowDefinition::from_json(&value.to_string()).unwrap()
}

#[test]
fn assign_replaces_enables_without_leaking_between_invocations() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let source = state_flow("%execute.enables");
    let plan = compile_definition(&source, &registry).unwrap();
    let authored: BTreeSet<_> = source.nodes.iter().map(|node| node.id.clone()).collect();
    let compiled: BTreeSet<_> = plan
        .graph
        .definition
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    assert_eq!(
        compiled, authored,
        "state assignment must not add graph nodes"
    );
    let target = &plan.graph.relations.writes[&"write".into()];
    let schema = mf_compiler::assignment_schema(&plan.graph, target).unwrap();
    assert!(
        mf_compiler::prepared_assign(&registry, "write", target, &schema)
            .unwrap()
            .into_task()
            .unwrap()
            .node
            .supports_success_dependencies()
    );
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (enables, route, active) in [
        (json!([]), "first", false),
        (json!(["second"]), "first", false),
        (json!(["second"]), "second", true),
        (json!(["first", "second"]), "first", true),
    ] {
        let output = flow
            .execute_with_inputs(
                WorkflowArguments::try_from(json!({"params.enables":enables,"params.route":route}))
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(output["result"].get("outputs").is_some(), active);
    }
    assert_eq!(
        plan.graph
            .definition
            .nodes
            .iter()
            .find(|node| node.id.as_str() == "execute")
            .unwrap()
            .config,
        json!({"routes":["first","second"],"enables":["first"]})
    );
}

#[test]
fn saved_router_assignments_use_the_target_configuration() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&state_flow("%execute.enables"), &registry).unwrap();
    plan.graph
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "execute")
        .unwrap()
        .config = json!({"routes":["second"]});
    plan.graph
        .children
        .get_mut(&"execute".into())
        .unwrap()
        .remove("first");
    let saved = mf_compiler::CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    let flow = instantiate_compiled(&saved, &registry).unwrap();
    assert_eq!(
        flow.execute_with_inputs(
            WorkflowArguments::try_from(json!({
                "params.enables":["second"], "params.route":"second"
            }))
            .unwrap()
        )
        .unwrap()["result"]["outputs"]["value"],
        json!(10)
    );
    let error = flow
        .execute_with_inputs(
            WorkflowArguments::try_from(json!({
                "params.enables":["first"], "params.route":"first"
            }))
            .unwrap(),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("subset of its routes"),
        "{error}"
    );
    #[cfg(feature = "codegen")]
    saved.generate_runner_execution_plans(&registry).unwrap();
}

#[test]
fn invalid_router_state_writes_remain_recoverable_packed_call_errors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for enables in [json!(["missing"]), json!(["first", "first"])] {
        let definition: WorkflowDefinition = serde_json::from_value(json!({
            "version":"2026-11-13", "nodes":[
                {"id":"body", "kind":"builtin.constant", "config":{
                    "type":"flow", "value":state_flow("%execute.enables")
                }},
                {"id":"tools", "kind":"builtin.object", "config":{"fields":{"state":"flow"}},
                    "inputs":{"state":edge("body", "value")}},
                {"id":"route", "kind":"builtin.constant", "config":{"value":"state"}},
                {"id":"arguments", "kind":"builtin.constant", "config":{"value":{"params.enables":enables}}},
                {"id":"invoke", "kind":"builtin.router", "config":{"routes":[]}, "inputs":{
                    "flows":edge("tools", "value"), "route":edge("route", "value"), "arguments":edge("arguments", "value")
                }}
            ], "outputs":[{"name":"result", "node":"invoke", "port":"result"}]
        }))
        .unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let output = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap();
        assert!(
            output["result"]["error"]
                .as_str()
                .unwrap()
                .contains("subset of its routes")
        );
    }
}

#[test]
fn router_assignment_config_rejects_extra_fields() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = state_flow("%execute.enables");
    source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "write")
        .unwrap()
        .config["extra"] = json!(true);
    let error = compile_definition(&source, &registry).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
}

#[test]
fn readonly_unknown_duplicate_and_invalid_node_state_writes_are_rejected() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for target in [
        "%execute.routes",
        "%execute.config.routes",
        "%missing.enables",
    ] {
        assert!(compile_definition(&state_flow(target), &registry).is_err());
    }
    let plan = compile_definition(&state_flow("%execute.enables"), &registry).unwrap();
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
