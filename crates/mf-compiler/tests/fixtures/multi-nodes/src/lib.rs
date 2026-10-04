use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, Outputs, PortSpec, TaskNode,
    ValueType,
};
use serde_json::{Value, json};

struct Source;

impl TaskNode for Source {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        if cfg!(feature = "fail-execution") {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "execution sentinel".into(),
            });
        }
        Ok((Outputs::from([(
            "value".into(),
            json!(if cfg!(feature = "double") { 14 } else { 7 }).into(),
        )]))
        .into())
    }
}

struct Echo;

impl TaskNode for Echo {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".into(), inputs["input"].clone())])).into())
    }
}

fn source(
    config: Value,
    declared_ports: mf_runtime::NodePorts,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    if config.get("print").and_then(Value::as_bool) == Some(true) {
        println!("factory diagnostic");
    }
    let _: serde_json::Map<String, Value> = mf_runtime::deserialize_config(config)?;
    let node = Source;
    let metadata = mf_runtime::NodeMetadata::new(declared_ports);
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

fn echo(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let _: serde_json::Map<String, Value> = mf_runtime::deserialize_config(config)?;
    let node = Echo;
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![PortSpec::new("input", ValueType::Number, true)],
        outputs: vec![PortSpec::new("value", ValueType::Number, true)],
    });
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "fixture.source", factory: mf_runtime::NodeFactory::Plain(|config| source(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "fixture.echo", factory: mf_runtime::NodeFactory::Plain(echo) }
}

#[cfg(feature = "duplicate-kind")]
inventory::submit! {
    NodeRegistration { kind: "fixture.source", factory: mf_runtime::NodeFactory::Plain(|config| source(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![] })) }
}

mod context_fixture;
mod line_producer;
mod stream_fixture;
mod typed_fixture;
