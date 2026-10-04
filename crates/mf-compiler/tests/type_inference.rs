mod common;
#[path = "fixtures/multi-nodes/src/typed_fixture.rs"]
mod fixture;
extern crate mfn_core as _;

use mf_compiler::{
    Inputs, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, NodeRegistry,
    OutputDerivation, Outputs, PortSpec, TaskNode, ValueType, WorkflowDefinition,
    compile_definition, instantiate_compiled,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

fn dependencies() -> Value {
    let mut dependencies = serde_json::to_value(common::fixture_definition().dependencies).unwrap();
    dependencies["core"] =
        json!({"package":"mfn-core","path":common::crates_dir().join("builtin-nodes/core")});
    dependencies
}

fn linear(value: Value, identities: usize, sink_type: &str) -> WorkflowDefinition {
    let mut nodes = vec![json!({"id":"source","kind":"builtin.constant","config":{"value":value}})];
    let mut edges = Vec::new();
    let mut previous = "source".to_owned();
    for index in 0..identities {
        let id = format!("identity_{index}");
        nodes.push(json!({"id":id,"kind":"builtin.identity"}));
        edges.push(
            json!({"from_node":previous,"from_output":"value","to_node":id,"to_input":"input"}),
        );
        previous = id;
    }
    nodes.push(json!({"id":"sink","kind":"fixture.typed_echo","config":{"type":sink_type}}));
    edges.push(
        json!({"from_node":previous,"from_output":"value","to_node":"sink","to_input":"input"}),
    );
    serde_json::from_value(json!({
        "version":"2026-09-26",
        "dependencies":dependencies(),
        "nodes":nodes,
        "edges":edges,
        "outputs":[{"name":"result","node":"sink","port":"value"}]
    }))
    .unwrap()
}

#[test]
fn infers_core_values_through_identity_chains() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (value, identities, sink_type) in [
        (json!(42), 2, "int64"),
        (json!([]), 1, "list_int64"),
        (json!({}), 1, "map_int64"),
        (json!([{"count":1},{"count":2}]), 2, "list_map_int64"),
        (json!(null), 1, "any"),
    ] {
        let definition = linear(value.clone(), identities, sink_type);
        let plan = compile_definition(&definition, &registry).unwrap();
        let result = instantiate_compiled(&plan, &registry)
            .unwrap()
            .execute()
            .unwrap();
        assert_eq!(result["result"], value);
    }
}

#[test]
fn rejects_known_conflicts_with_edge_and_nested_path_context() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (value, identities, sink_type, path) in [
        (json!(42), 0, "string", "path ``"),
        (json!(null), 2, "int64", "path ``"),
        (json!([1, "x"]), 1, "list_int64", "path `/1`"),
        (json!({"a":1,"b":"x"}), 2, "map_int64", "path `/b`"),
        (json!(u64::MAX), 1, "int64", "unsigned integer"),
    ] {
        let error = compile_definition(&linear(value, identities, sink_type), &registry)
            .unwrap_err()
            .to_string();
        assert!(error.contains("sink") && error.contains("input"), "{error}");
        assert!(
            error.contains("known value") && error.contains(path),
            "{error}"
        );
    }
}

#[test]
fn propagates_declared_plugin_types_without_a_known_value() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = linear(json!(null), 1, "list_int64");
    definition.nodes[0].kind = "fixture.typed_source".into();
    definition.nodes[0].config = json!({"type":"list_string","value":["x"]});
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("identity_0") && error.contains("sink"),
        "{error}"
    );
    assert!(
        error.contains("list<string>") && error.contains("list<int64>"),
        "{error}"
    );

    definition.nodes[0].config = json!({"type":"list_int64","value":[1,2]});
    let plan = compile_definition(&definition, &registry).unwrap();
    let result = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(result["result"], json!([1, 2]));
}

struct ForwardingNode;

impl TaskNode for ForwardingNode {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".into(), inputs["input"].clone())])).into())
    }
}
impl ForwardingNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Any, true)],
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        }
    }
    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::forward_input("value", "input")]
    }
}

fn forwarding_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = ForwardingNode;
    let metadata = mf_runtime::NodeMetadata {
        ports: node.ports(),
        output_derivations: node.output_derivations(),
        stdin: None,
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.forwarding", factory: mf_runtime::NodeFactory::Plain(forwarding_factory) }
}

#[test]
fn rejects_forwarded_output_conflicts() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-09-26",
        "dependencies":dependencies(),
        "nodes":[
            {"id":"source","kind":"builtin.constant","config":{"value":"wrong"}},
            {"id":"forward","kind":"fixture.forwarding"}
        ],
        "edges":[{"from_node":"source","from_output":"value","to_node":"forward","to_input":"input"}],
        "outputs":[{"name":"result","node":"forward","port":"value"}]
    }))
    .unwrap();
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("forward") && error.contains("known value"),
        "{error}"
    );

    definition.nodes[0].kind = "fixture.typed_source".into();
    definition.nodes[0].config = json!({"type":"string","value":"wrong"});
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("forward") && error.contains("declares int64"),
        "{error}"
    );
}

#[test]
fn control_edges_do_not_propagate_known_data() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-09-26",
        "dependencies":dependencies(),
        "nodes":[
            {"id":"source","kind":"fixture.typed_source","config":{"type":"any","value":"bad"}},
            {"id":"control","kind":"builtin.constant","config":{"value":42}},
            {"id":"identity","kind":"builtin.identity"},
            {"id":"sink","kind":"fixture.typed_echo","config":{"type":"int64"}}
        ],
        "edges":[
            {"from_node":"source","from_output":"value","to_node":"identity","to_input":"input"},
            {"from_node":"identity","from_output":"value","to_node":"sink","to_input":"input"}
        ],
        "control_edges":[{"from_node":"control","from_output":"value","to_node":"identity"}],
        "outputs":[{"name":"result","node":"sink","port":"value"}]
    }))
    .unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let error = instantiate_compiled(&plan, &registry)
        .unwrap()
        .execute()
        .unwrap_err()
        .to_string();
    assert!(error.contains("sink") && error.contains("int64"), "{error}");
}

static INVALID_EXECUTIONS: AtomicUsize = AtomicUsize::new(0);

struct InvalidMetadataNode;

impl TaskNode for InvalidMetadataNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        INVALID_EXECUTIONS.fetch_add(1, Ordering::Relaxed);
        Ok((Outputs::from([("value".into(), json!("wrong").into())])).into())
    }
}
impl InvalidMetadataNode {
    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::forward_input("value", "missing")]
    }
}

fn invalid_metadata_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = InvalidMetadataNode;
    let metadata = mf_runtime::NodeMetadata {
        ports: mf_runtime::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
        },
        output_derivations: node.output_derivations(),
        stdin: None,
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.invalid_metadata", factory: mf_runtime::NodeFactory::Plain(invalid_metadata_factory) }
}

#[test]
fn rejects_bad_derivations_without_executing_nodes() {
    INVALID_EXECUTIONS.store(0, Ordering::Relaxed);
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-09-26",
        "dependencies":{},
        "nodes":[{"id":"bad","kind":"fixture.invalid_metadata"}],
        "outputs":[]
    }))
    .unwrap();
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("bad") && error.contains("missing"),
        "{error}"
    );
    assert_eq!(INVALID_EXECUTIONS.load(Ordering::Relaxed), 0);
}
