use mf_runtime::{
    ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode, TaskNode, ValueType,
};
use std::sync::atomic::{AtomicU64, Ordering};

struct Counter(AtomicU64);

impl TaskNode for Counter {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let count = self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Outputs::from([("value".into(), count.into())]).into())
    }
}

fn prepare(config: serde_json::Value) -> Result<PreparedNode, NodeBuildError> {
    let _: serde_json::Map<String, serde_json::Value> = mf_runtime::deserialize_config(config)?;
    Ok(PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Counter(AtomicU64::new(0)))),
        NodeMetadata::new(NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new("value", ValueType::Uint64, true)],
        }),
    ))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.embedding_counter", factory: NodeFactory::Plain(prepare) }
}
