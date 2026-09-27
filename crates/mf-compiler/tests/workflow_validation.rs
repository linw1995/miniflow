use mf_compiler::WorkflowDefinition;
use mf_compiler::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs,
    PortSpec, ValueType, WorkflowCompileError, validate_definition,
};
use serde_json::{Value, json};

struct NoopNode;

impl Node for NoopNode {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::new())
    }
}

struct DeepTypeNode;

impl Node for DeepTypeNode {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::new())
    }

    fn ports(&self) -> Option<mf_compiler::NodePorts> {
        let mut value_type = ValueType::Int64;
        for _ in 0..ValueType::MAX_DEPTH {
            value_type = ValueType::List(Box::new(value_type));
        }
        Some(mf_compiler::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::owned("value", value_type, true)],
        })
    }
}

fn noop_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(NoopNode))
}

fn deep_type_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(DeepTypeNode))
}

inventory::submit! {
    NodeRegistration {
        kind: "source.deep_type",
        inputs: &[],
        outputs: &[],
        factory: deep_type_factory,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "source.number",
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::Number, true)],
        factory: noop_factory,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "source.string",
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::String, true)],
        factory: noop_factory,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "source.any",
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::Any, true)],
        factory: noop_factory,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "sink.number",
        inputs: &[PortSpec::new("input", ValueType::Number, true)],
        outputs: &[PortSpec::new("result", ValueType::Number, true)],
        factory: noop_factory,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "sink.any",
        inputs: &[PortSpec::new("input", ValueType::Any, true)],
        outputs: &[PortSpec::new("result", ValueType::Any, true)],
        factory: noop_factory,
    }
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
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::IncompatiblePortTypes { .. }
    ));

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
