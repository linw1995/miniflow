use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, Outputs, PortSpec,
    ValueType,
};
use serde_json::{Value, json};

struct Source;

impl Node for Source {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        if cfg!(feature = "fail-execution") {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "execution sentinel".into(),
            });
        }
        Ok(Outputs::from([(
            "value".into(),
            json!(if cfg!(feature = "double") { 14 } else { 7 }),
        )]))
    }
}

struct Echo;

impl Node for Echo {
    fn execute(&self, inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([("value".into(), inputs["input"].clone())]))
    }
}

fn source(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    if config.get("print").and_then(Value::as_bool) == Some(true) {
        println!("factory diagnostic");
    }
    let _: serde_json::Map<String, Value> = mf_runtime::deserialize_config(config)?;
    Ok(Box::new(Source))
}

fn echo(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let _: serde_json::Map<String, Value> = mf_runtime::deserialize_config(config)?;
    Ok(Box::new(Echo))
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.source",
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::Number, true)],
        factory: source,
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.echo",
        inputs: &[PortSpec::new("input", ValueType::Number, true)],
        outputs: &[PortSpec::new("value", ValueType::Number, true)],
        factory: echo,
    }
}

#[cfg(feature = "duplicate-kind")]
inventory::submit! {
    NodeRegistration { kind: "fixture.source", inputs: &[], outputs: &[], factory: source }
}

mod context_fixture;
mod typed_fixture;
