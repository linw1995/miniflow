use mf_compiler::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs, PortSpec,
    TaskNode, ValueType,
};
use serde_json::{Value, json};

struct SourceNode;

impl TaskNode for SourceNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".to_owned(), json!(7).into())])).into())
    }
}

struct SinkNode;

impl TaskNode for SinkNode {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("received".to_owned(), inputs["value"].clone())])).into())
    }
}

fn source_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = SourceNode;
    let metadata = mf_runtime::NodeMetadata {
        ports: mf_runtime::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Number, true)],
        },
        output_derivations: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

fn sink_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = SinkNode;
    let metadata = mf_runtime::NodeMetadata {
        ports: mf_runtime::NodePorts {
            inputs: vec![PortSpec::new("value", ValueType::Number, true)],
            outputs: vec![PortSpec::new("received", ValueType::Number, true)],
        },
        output_derivations: Vec::new(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "test.source", factory: mf_runtime::NodeFactory::Plain(source_factory) }
}

inventory::submit! {
    NodeRegistration { kind: "test.sink", factory: mf_runtime::NodeFactory::Plain(sink_factory) }
}

#[test]
fn discovers_linked_node_registrations_and_creates_nodes() {
    let registry = NodeRegistry::from_inventory().unwrap();

    let source = registry.get("test.source").unwrap();
    let source_node = source.instantiate(Value::Null).unwrap();
    let source_node = source_node
        .execution
        .into_task_node()
        .expect("expected task");
    let source_outputs = source_node
        .execute(Inputs::new(), &mut mf_runtime::ExecutionContext::default())
        .unwrap()
        .outputs;
    assert_eq!(source_outputs["value"], json!(7));

    let sink = registry.get("test.sink").unwrap();
    let sink_node = sink.instantiate(Value::Null).unwrap();
    let sink_node = sink_node.execution.into_task_node().expect("expected task");
    let sink_outputs = sink_node
        .execute(
            Inputs::from([("value".to_owned(), source_outputs["value"].clone())]),
            &mut mf_runtime::ExecutionContext::default(),
        )
        .unwrap()
        .outputs;
    assert_eq!(sink_outputs["received"], json!(7));
}
