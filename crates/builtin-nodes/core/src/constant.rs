use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, Outputs, PortSpec,
    ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;

pub const KIND: &str = "builtin.constant";

pub fn kind() -> &'static str {
    KIND
}

#[derive(Deserialize)]
struct ConstantConfig {
    value: Value,
}

struct ConstantNode {
    value: Value,
}

impl Node for ConstantNode {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::from([("value".to_owned(), self.value.clone())]))
    }
}

fn constant_factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let config: ConstantConfig = deserialize_config(config)?;
    Ok(Box::new(ConstantNode {
        value: config.value,
    }))
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[],
        outputs: &[PortSpec::new("value", ValueType::Any, true)],
        factory: constant_factory,
    }
}
