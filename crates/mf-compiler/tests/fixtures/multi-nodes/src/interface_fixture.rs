use mf_runtime::{
    Emitter, ExecutionContext, Inputs, NodeBuildError, NodeExecutionError, NodeFactory,
    NodeMetadata, NodePorts, NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode,
    StdinRequirement, StreamNode, TaskNode, ValueType,
};
use serde_json::Value;

struct Contract;
impl TaskNode for Contract {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(path) = std::env::var_os("MF_FIXTURE_EXECUTION_MARKER") {
            // A test fixture records dispatch without consuming any source data.
            std::fs::write(path, b"executed").unwrap();
        }
        Ok(Outputs::from([("value".into(), "executed".into())]).into())
    }
}
impl StreamNode for Contract {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        emitter.send(TaskNode::execute(self, inputs, context)?)
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let mode = std::env::var("MF_FIXTURE_INTERFACE_CHANGE").unwrap_or_default();
    if mode == "initialization" {
        let _: bool = mf_runtime::deserialize_config(Value::Null)?;
    }
    let mut inputs = vec![PortSpec::new("path./~", ValueType::String, false)];
    let mut stdin = None;
    match mode.as_str() {
        "type" => inputs[0].value_type = ValueType::Int64,
        "required" => inputs[0].required = true,
        "remove" => inputs.clear(),
        "add" => inputs.push(PortSpec::new("extra", ValueType::String, false)),
        "stdin" => stdin = Some(StdinRequirement::Always),
        "condition" => stdin = Some(StdinRequirement::UnlessInput("path./~".into())),
        _ => {}
    }
    let metadata = NodeMetadata {
        stdin,
        ..NodeMetadata::new(NodePorts {
            inputs,
            outputs: vec![PortSpec::new("value", ValueType::String, true)],
        })
    };
    Ok(if config["stream"].as_bool().unwrap_or(false) {
        PreparedNode::stream(Contract, metadata)
    } else {
        PreparedNode::new(Contract, metadata)
    })
}

inventory::submit! { NodeRegistration { kind: "fixture.startup_contract", factory: NodeFactory::Plain(factory) } }
