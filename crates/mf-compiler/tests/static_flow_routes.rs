//! Static router candidates preserve the caller's ordinary native-map wiring.
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
use snafu::ResultExt;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

static FIRST: AtomicUsize = AtomicUsize::new(0);
static SECOND: AtomicUsize = AtomicUsize::new(0);
static UNDECLARED: AtomicUsize = AtomicUsize::new(0);

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
        assert!(
            context
                .invocation_path()
                .last()
                .unwrap()
                .node_id()
                .contains("execute")
        );
        Ok(Outputs::from([("value".into(), inputs["value"].clone())]).into())
    }
}
inventory::submit! {
    NodeRegistration { kind: "test.static-route-effect", factory: NodeFactory::Plain(|config| {
        PreparedNode::new(Effect { first: config["first"] == true }, NodeMetadata::default())
    }) }
}

/// No provenance callback: filtering conservatively retains all incoming candidates.
struct SelectTools {
    records: bool,
}
impl NodePortContract for SelectTools {
    fn ports(&self) -> NodePorts {
        let tools = ValueType::Map(Box::new(if self.records {
            ValueType::Object
        } else {
            ValueType::Flow
        }));
        NodePorts {
            inputs: vec![
                PortSpec::new("tools", tools.clone(), true),
                PortSpec::new("enabled", ValueType::Int64, true),
            ],
            outputs: vec![PortSpec::new("value", tools, true)],
        }
    }
}
impl TaskNode for SelectTools {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let enabled = inputs["enabled"].as_i64().unwrap();
        let tools = &inputs["tools"];
        let entries = match enabled {
            0 => vec![],
            1 => vec![("first".into(), tools["first"].clone())],
            2 => vec![("second".into(), tools["second"].clone())],
            4 => vec![("first".into(), tools["second"].clone())],
            5 => vec![("new".into(), tools["first"].clone())],
            6 => {
                let flow = mf_runtime::FlowValue::new(mf_runtime::PreparedSubgraph::new(
                    vec![],
                    vec![],
                    |_| {
                        UNDECLARED.fetch_add(1, Ordering::SeqCst);
                        Ok(Default::default())
                    },
                ))
                .boxed()
                .context(mf_runtime::NodePluginFailedSnafu)?;
                vec![("first".into(), ValueRef::from(flow))]
            }
            _ => tools
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        };
        Ok(Outputs::from([("value".into(), ValueRef::object(entries))]).into())
    }
}
inventory::submit! {
    NodeRegistration { kind: "test.select-static-tools", factory: NodeFactory::Plain(|config| {
        PreparedNode::new(SelectTools { records: config["records"] == true }, NodeMetadata::default())
    }) }
}

struct CounterEffect(AtomicUsize);
impl NodePortContract for CounterEffect {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        }
    }
}
impl TaskNode for CounterEffect {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        Ok(Outputs::from([(
            "value".into(),
            ValueRef::from(self.0.fetch_add(1, Ordering::SeqCst) as i64 + 1),
        )])
        .into())
    }
}
inventory::submit! {
    NodeRegistration { kind:"test.shared-route-counter", factory:NodeFactory::Plain(|_| {
        PreparedNode::new(CounterEffect(AtomicUsize::new(0)), NodeMetadata::default())
    }) }
}

struct AnonymousSource;
impl NodePortContract for AnonymousSource {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Flow, true)],
        }
    }
}
impl TaskNode for AnonymousSource {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let value =
            mf_runtime::FlowValue::new(mf_runtime::PreparedSubgraph::new(vec![], vec![], |_| {
                UNDECLARED.fetch_add(1, Ordering::SeqCst);
                Ok(Default::default())
            }))
            .boxed()
            .context(mf_runtime::NodePluginFailedSnafu)?;
        Ok(Outputs::from([("value".into(), value.into())]).into())
    }
}
inventory::submit! {
    NodeRegistration { kind: "test.anonymous-static-source", factory: NodeFactory::Plain(|_| PreparedNode::new(AnonymousSource, NodeMetadata::default())) }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.declared-static-source",
        factory: NodeFactory::WithFlowBindings {
            factory: &NodeFactory::Subgraph {
                scope: None,
                flow_parameters: |config| BTreeMap::from([("body".into(), config["body"].clone())]),
                plain: |_| mf_runtime::NodeInvalidSubgraphSnafu { message:"a prepared body is required" }.fail(),
                prepared: |id, config, mut bodies| {
                    NodeRegistry::from_inventory().unwrap().get("builtin.constant").unwrap().instantiate_subgraphs(
                        id, json!({"type":"flow", "value":config["body"]}),
                        BTreeMap::from([("value".into(), bodies.remove("body").unwrap())]),
                    )
                },
            },
            bindings: |_| mf_runtime::FlowBindings {
                outputs:BTreeMap::from([("value".into(), mf_runtime::FlowBindingSource::Child("body".into()))]),
                ..Default::default()
            },
        },
    }
}

fn edge(node: &str, port: &str) -> Value {
    json!({"from":{"node":node,"port":port}})
}
fn effect(first: bool) -> Value {
    json!({"version":"2026-11-13", "nodes":[{"id":"effect", "kind":"test.static-route-effect", "config":{"first":first}}],
        "outputs":[{"name":"value", "node":"effect", "port":"value"}]})
}
fn definition(records: bool) -> WorkflowDefinition {
    let mut nodes = vec![
        json!({"id":"first", "kind":"builtin.constant", "config":{"type":"flow", "value":effect(true)}}),
        json!({"id":"second", "kind":"builtin.constant", "config":{"type":"flow", "value":effect(false)}}),
        json!({"id":"params", "kind":"builtin.parameters", "config":{"parameters":{
            "enabled":{"type":"int", "default":3, "secret":false},
            "route":{"type":"string", "default":"first", "secret":false},
            "value":{"type":"int", "default":10, "secret":false}
        }}}),
    ];
    if records {
        for name in ["first", "second"] {
            nodes.push(json!({"id":format!("{name}_record"), "kind":"builtin.object", "config":{"fields":{"run":"flow"}}, "inputs":{"run":edge(name,"value")}}));
        }
    }
    let fields = if records {
        json!({"first":"object", "second":"object"})
    } else {
        json!({"first":"flow", "second":"flow"})
    };
    let inputs = if records {
        json!({"first":edge("first_record","value"), "second":edge("second_record","value")})
    } else {
        json!({"first":edge("first","value"), "second":edge("second","value")})
    };
    nodes.extend([
        json!({"id":"tools", "kind":"builtin.object", "config":{"fields":fields}, "inputs":inputs}),
        json!({"id":"select", "kind":"test.select-static-tools", "config":{"records":records}, "inputs":{"tools":edge("tools","value"), "enabled":edge("params","enabled")}}),
        json!({"id":"arguments", "kind":"builtin.object", "config":{"fields":{"value":"int"}}, "inputs":{"value":edge("params","value")}}),
        json!({"id":"execute", "kind":"builtin.router", "config":if records {json!({"mode":"execute", "flow_field":"run"})} else {json!({"mode":"execute"})},
            "inputs":{"route":edge("params","route"), "flows":edge("select","value"), "arguments":edge("arguments","value")}}),
    ]);
    WorkflowDefinition::from_json(&json!({"version":"2026-11-13", "nodes":nodes, "outputs":[{"name":"result", "node":"execute", "port":"result"}]}).to_string()).unwrap()
}
fn arguments(enabled: i64, route: &str, value: i64) -> WorkflowArguments {
    WorkflowArguments::try_from(
        json!({"params.enabled":enabled, "params.route":route, "params.value":value}),
    )
    .unwrap()
}

#[test]
fn provider_metadata_not_node_names_or_flow_port_types_authorizes_candidates() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for declared in [true, false] {
        let mut source = definition(false);
        for node in &mut source.nodes {
            if matches!(node.id.as_str(), "first" | "second") {
                node.kind = if declared {
                    "test.declared-static-source"
                } else {
                    "test.anonymous-static-source"
                }
                .into();
                node.config = if declared {
                    json!({"body":node.config["value"]})
                } else {
                    json!({"body":{"path":"must-not-read.json"}})
                };
            }
        }
        let plan = compile_definition(&source, &registry).unwrap();
        assert_eq!(
            describe_compiled(&plan)
                .unwrap()
                .body_paths_for_node(&[], "execute")
                .len(),
            if declared { 2 } else { 0 }
        );
        let result = instantiate_compiled(&plan, &registry).unwrap().execute();
        if declared {
            assert_eq!(result.unwrap()["result"]["outputs"]["value"], json!(10));
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("outside its compiled Flow superset")
            );
        }
    }
    assert_eq!(UNDECLARED.load(Ordering::SeqCst), 0);
}

#[test]
fn enabling_removing_and_readding_tools_does_not_change_the_compiled_superset() {
    for records in [false, true] {
        let registry = NodeRegistry::from_inventory().unwrap();
        let source = definition(records);
        assert!(
            source
                .nodes
                .iter()
                .find(|node| node.id.as_str() == "execute")
                .unwrap()
                .config
                .get("flow_candidates")
                .is_none()
        );
        let plan = compile_definition(&source, &registry).unwrap();
        let description = describe_compiled(&plan).unwrap();
        assert_eq!(description.body_paths_for_node(&[], "execute").len(), 2);
        assert_eq!(plan.graph.children[&"execute".into()].len(), 2);
        let restored = mf_runtime::WorkflowManifest::from_bytes(
            &plan.manifest(&registry).unwrap().to_bytes().unwrap(),
        )
        .unwrap();
        assert_eq!(restored.description().unwrap(), description);
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        for (enabled, route, value) in [
            (1, "first", 11),
            (2, "second", 22),
            (3, "first", 33),
            (1, "first", 44),
        ] {
            assert_eq!(
                flow.execute_with_inputs(arguments(enabled, route, value))
                    .unwrap()["result"]["outputs"]["value"],
                json!(value)
            );
        }
        for (enabled, route) in [(0, "first"), (1, "second"), (2, "first")] {
            assert!(
                flow.execute_with_inputs(arguments(enabled, route, 0))
                    .unwrap_err()
                    .to_string()
                    .contains("has no Flow route")
            );
        }
        assert_eq!(describe_compiled(&plan).unwrap(), description);
    }
    assert_eq!(FIRST.load(Ordering::SeqCst), 6);
    assert_eq!(SECOND.load(Ordering::SeqCst), 2);
}

#[test]
fn candidate_freezing_does_not_overwrite_invalid_authored_configuration() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for config in [
        json!({"mode":"execute", "flow_candidates":false}),
        json!({"mode":"execute", "flow_candidate_0":{"path":"must-not-read.json"}}),
        json!({"mode":"execute", "flow_candidates":{"flow_candidate_0":{"route":"first", "source":[]}}, "flow_candidate_0":{"path":"must-not-read.json"}}),
        json!({"mode":"execute", "flow_field":17}),
    ] {
        let mut source = definition(false);
        source
            .nodes
            .iter_mut()
            .find(|node| node.id.as_str() == "execute")
            .unwrap()
            .config = config;
        let error = compile_definition(&source, &registry).unwrap_err();
        assert!(
            matches!(
                error,
                mf_compiler::WorkflowCompileError::NodeConstruction { .. }
            ),
            "{error}"
        );
        assert!(std::error::Error::source(&error).is_some());
        assert!(!error.to_string().contains("could not read"), "{error}");
    }
}

#[test]
fn a_new_name_or_substituted_capability_cannot_expand_the_superset() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(false), &registry).unwrap();
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (enabled, route) in [(4, "first"), (5, "new"), (6, "first")] {
        let error = flow
            .execute_with_inputs(arguments(enabled, route, 0))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outside its compiled Flow superset"),
            "{error}"
        );
        assert!(std::error::Error::source(&error).is_some());
    }
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
    assert_eq!(SECOND.load(Ordering::SeqCst), 0);
    assert_eq!(UNDECLARED.load(Ordering::SeqCst), 0);
}

#[test]
fn compiled_candidates_still_validate_runtime_argument_types_before_effects() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition(false);
    let arguments = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "arguments")
        .unwrap();
    arguments.config["fields"]["value"] = json!("string");
    let params = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "params")
        .unwrap();
    params.config["parameters"]["value"] =
        json!({"type":"string", "default":"wrong", "secret":false});
    let plan = compile_definition(&source, &registry).unwrap();
    assert_eq!(
        describe_compiled(&plan)
            .unwrap()
            .body_paths_for_node(&[], "execute")
            .len(),
        2
    );
    let result = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert!(result["result"]["error"].as_str().unwrap().contains("int"));
    assert!(result["result"].is_secret());
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
}

#[test]
fn multiple_invokers_reuse_the_selected_capability_preparation() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(definition(false)).unwrap();
    let nodes = value["nodes"].as_array_mut().unwrap();
    for node in nodes.iter_mut().filter(|node| node["id"] == "first") {
        node["config"]["value"]["nodes"][0]["kind"] = json!("test.shared-route-counter");
    }
    let mut second = nodes
        .iter()
        .find(|node| node["id"] == "execute")
        .unwrap()
        .clone();
    second["id"] = json!("execute_again");
    nodes.push(second);
    value["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"execute", "to_node":"execute_again"}));
    value["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"again", "node":"execute_again", "port":"result"}));
    let source = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let plan = compile_definition(&source, &registry).unwrap();
    let description = describe_compiled(&plan).unwrap();
    for owner in ["execute", "execute_again"] {
        assert_eq!(description.body_paths_for_node(&[], owner).len(), 2);
    }
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for expected in [1, 3] {
        let output = flow.execute().unwrap();
        assert_eq!(output["result"]["outputs"]["value"], json!(expected));
        assert_eq!(output["again"]["outputs"]["value"], json!(expected + 1));
    }
}

#[test]
fn ordinary_startup_nodes_forward_candidates_through_configured_calls() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition(false);
    let select = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "select")
        .unwrap();
    select.kind = "workflow.call".into();
    select.config = json!({"flow":"filter", "body":{
        "version":"2026-11-13", "nodes":[{"id":"filter", "kind":"test.select-static-tools", "config":{"records":false}}],
        "outputs":[{"name":"value", "node":"filter", "port":"value"}]
    }});
    for edge in &mut source.edges {
        if edge.to_node.as_str() == "select" {
            edge.to_input = format!("filter.{}", edge.to_input);
        }
    }
    let plan = compile_definition(&source, &registry).unwrap();
    assert_eq!(
        describe_compiled(&plan)
            .unwrap()
            .body_paths_for_node(&[], "execute")
            .len(),
        2
    );
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute_with_inputs(arguments(2, "second", 19))
        .unwrap();
    assert_eq!(output["result"]["outputs"]["value"], json!(19));
    assert_eq!(FIRST.load(Ordering::SeqCst), 0);
    assert_eq!(SECOND.load(Ordering::SeqCst), 1);
}

#[test]
fn workflow_state_assignments_preserve_the_superset_across_runtime_filtering() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut value = serde_json::to_value(definition(false)).unwrap();
    value["variables"] = json!([{"name":"available", "type":{"map":"flow"}, "initial":{}}]);
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id":"write", "kind":"workflow.assign", "config":{"variable":"available"},
        "inputs":{"value":edge("select","value")}
    }));
    for edge in value["edges"].as_array_mut().unwrap() {
        if edge["to_node"] == "execute" && edge["to_input"] == "flows" {
            edge["from_node"] = json!("%root");
            edge["from_output"] = json!("available");
        }
    }
    value["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from_node":"write", "from_output":"done", "to_node":"execute"}));
    let source = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let plan = compile_definition(&source, &registry).unwrap();
    assert_eq!(
        describe_compiled(&plan)
            .unwrap()
            .body_paths_for_node(&[], "execute")
            .len(),
        2
    );
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    for (enabled, route) in [(1, "first"), (2, "second"), (3, "first")] {
        assert_eq!(
            flow.execute_with_inputs(arguments(enabled, route, 12))
                .unwrap()["result"]["outputs"]["value"],
            json!(12)
        );
    }
}

#[test]
fn nested_router_candidates_preserve_producer_origins_and_static_execution_scopes() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut source = definition(false);
    let nested = definition(false);
    let producer = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "first")
        .unwrap();
    producer.config["value"] = serde_json::to_value(nested).unwrap();
    // The nested Flow's sole startup node is params, whose defaults enable first.
    let arguments = source
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "arguments")
        .unwrap();
    arguments.config["fields"] = json!({});
    source
        .edges
        .retain(|edge| edge.to_node.as_str() != "arguments");
    let plan = compile_definition(&source, &registry).unwrap();
    let outer = &plan.graph.children[&"execute".into()];
    let nested = outer
        .values()
        .find(|child| child.children.contains_key(&"execute".into()))
        .unwrap();
    assert_eq!(nested.children[&"execute".into()].len(), 2);
    let output = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        output["result"]["outputs"]["result"]["outputs"]["value"],
        json!(10)
    );
    assert_eq!(FIRST.load(Ordering::SeqCst), 1);
}
