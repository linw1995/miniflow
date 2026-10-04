use mf_compiler::WorkflowDefinition;
use mf_compiler::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs, PortSpec,
    TaskNode, ValueType, WorkflowCompileError, validate_definition,
};
use serde_json::{Value, json};

struct NoopNode;

impl TaskNode for NoopNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok(Outputs::new().into())
    }
}

struct DeepTypeNode;

impl TaskNode for DeepTypeNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok(Outputs::new().into())
    }
}
impl DeepTypeNode {
    fn ports(&self) -> mf_compiler::NodePorts {
        let mut value_type = ValueType::Int64;
        for _ in 0..ValueType::MAX_DEPTH {
            value_type = ValueType::List(Box::new(value_type));
        }
        mf_compiler::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::owned("value", value_type, true)],
        }
    }
}

fn noop_factory(
    _config: Value,
    declared_ports: mf_runtime::NodePorts,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = NoopNode;
    let metadata = mf_runtime::NodeMetadata {
        ports: declared_ports,
        output_derivations: Vec::new(),
        stdin: None,
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

fn deep_type_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = DeepTypeNode;
    let metadata = mf_runtime::NodeMetadata {
        ports: node.ports(),
        output_derivations: Vec::new(),
        stdin: None,
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "source.deep_type", factory: mf_runtime::NodeFactory::Plain(deep_type_factory) }
}

inventory::submit! {
    NodeRegistration { kind: "source.number", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "source.string", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::String, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "source.any", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::Any, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "sink.number", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![PortSpec::new("input", ValueType::Number, true)], outputs: vec![PortSpec::new("result", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "sink.any", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![PortSpec::new("input", ValueType::Any, true)], outputs: vec![PortSpec::new("result", ValueType::Any, true)] })) }
}

fn valid_definition() -> Value {
    json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [
            {"id": "source", "kind": "source.number"},
            {"id": "sink", "kind": "sink.number"}
        ],
        "edges": [{
            "from_node": "source",
            "from_output": "value",
            "to_node": "sink",
            "to_input": "input"
        }],
        "outputs": [{"name": "final", "node": "sink", "port": "result"}]
    })
}

fn validation_error(value: Value) -> WorkflowCompileError {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    validate_definition(&definition, &registry).unwrap_err()
}

#[test]
fn accepts_valid_connections_and_specific_values_into_any_inputs() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    validate_definition(&definition, &registry).unwrap();

    let mut value = valid_definition();
    value["nodes"][1]["kind"] = json!("sink.any");
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    validate_definition(&definition, &registry).unwrap();
}

#[test]
fn rejects_blank_and_duplicate_node_ids() {
    let mut value = valid_definition();
    value["nodes"][0]["id"] = json!("   ");
    let error = validation_error(value);
    assert!(matches!(
        &error,
        WorkflowCompileError::InvalidNodeId { position: 1 }
    ));
    assert!(error.to_string().contains("position 1"));

    let mut value = valid_definition();
    value["nodes"][1]["id"] = json!("source");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::DuplicateNodeId { .. }
    ));
}

#[test]
fn rejects_unknown_edge_endpoints_and_ports() {
    let mut value = valid_definition();
    value["edges"][0]["from_node"] = json!("missing-source");
    let error = validation_error(value);
    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownEdgeSource { .. }
    ));
    assert!(error.to_string().contains("missing-source"));

    let mut value = valid_definition();
    value["edges"][0]["to_node"] = json!("missing-target");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownEdgeTarget { .. }
    ));

    let mut value = valid_definition();
    value["edges"][0]["from_output"] = json!("absent");
    let error = validation_error(value);
    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownOutputPort { .. }
    ));
    assert!(error.to_string().contains("`source`.`absent`"));
    assert!(error.to_string().contains("`sink`.`input`"));

    let mut value = valid_definition();
    value["edges"][0]["to_input"] = json!("absent");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownInputPort { .. }
    ));
}

#[test]
fn rejects_incompatible_or_ambiguous_inputs() {
    let mut value = valid_definition();
    value["nodes"][0]["kind"] = json!("source.string");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::IncompatiblePortTypes { .. }
    ));

    let mut value = valid_definition();
    value["nodes"][0]["kind"] = json!("source.any");
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    validate_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap();

    let mut value = valid_definition();
    let duplicate = value["edges"][0].clone();
    value["edges"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::DuplicateInputConnection { .. }
    ));

    let mut value = valid_definition();
    value["edges"] = json!([]);
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::MissingRequiredInput { .. }
    ));
}

#[test]
fn rejects_invalid_selected_outputs() {
    let mut value = valid_definition();
    value["outputs"][0]["node"] = json!("missing");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownWorkflowOutputNode { .. }
    ));

    let mut value = valid_definition();
    value["outputs"][0]["port"] = json!("missing");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownWorkflowOutputPort { .. }
    ));

    let mut value = valid_definition();
    let duplicate = value["outputs"][0].clone();
    value["outputs"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::DuplicateWorkflowOutputName { .. }
    ));
}

#[test]
fn rejects_excessively_nested_instance_port_types() {
    let mut value = valid_definition();
    value["nodes"][0]["kind"] = json!("source.deep_type");
    let error = validation_error(value);
    assert!(matches!(
        error,
        WorkflowCompileError::InvalidNodeMetadata { .. }
    ));
    let message = error.to_string();
    assert!(message.contains("source"));
    assert!(message.contains("output port `value`"));
    assert!(message.contains("type nesting depth"));
}
