use mf_compiler::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs,
    PortSpec, ValueType,
};
use serde_json::{Value, json};

struct SourceNode;

impl Node for SourceNode {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([("value".to_owned(), json!(7).into())]))
    }
}

struct SinkNode;

impl Node for SinkNode {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([(
            "received".to_owned(),
            inputs["value"].clone(),
        )]))
    }
}

fn source_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(SourceNode))
}

fn sink_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(SinkNode))
}

inventory::submit! {
    NodeRegistration {
        kind: "test.source",
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::Number, true)],
        factory: source_factory,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "test.sink",
        inputs: &[PortSpec::new("value", ValueType::Number, true)],
        outputs: &[PortSpec::new("received", ValueType::Number, true)],
        factory: sink_factory,
    }
}

#[test]
fn discovers_linked_node_registrations_and_creates_nodes() {
    let registry = NodeRegistry::from_inventory().unwrap();

    let source = registry.get("test.source").unwrap();
    let source_node = source.instantiate(Value::Null).unwrap();
    let source_outputs = source_node.execute(Inputs::new()).unwrap();
    assert_eq!(source_outputs["value"], json!(7));

    let sink = registry.get("test.sink").unwrap();
    let sink_node = sink.instantiate(Value::Null).unwrap();
    let sink_outputs = sink_node
        .execute(Inputs::from([(
            "value".to_owned(),
            source_outputs["value"].clone(),
        )]))
        .unwrap();
    assert_eq!(sink_outputs["received"], json!(7));
}
