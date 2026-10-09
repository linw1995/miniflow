use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeRegistration, NodeValue, OutputDerivation,
    TypedNodeResult, TypedTaskNode, ValueRef, deserialize_config,
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
    value: ValueRef,
}

#[derive(NodeValue)]
struct ConstantInputs {}

#[derive(NodeValue)]
struct ConstantOutputs {
    value: ValueRef,
}

impl TypedTaskNode for ConstantNode {
    type Input = ConstantInputs;
    type Output = ConstantOutputs;

    fn execute(
        &self,
        _inputs: ConstantInputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<TypedNodeResult<ConstantOutputs>, NodeExecutionError> {
        Ok(ConstantOutputs {
            value: self.value.clone(),
        }
        .into())
    }
}

fn constant_factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: ConstantConfig = deserialize_config(config)?;
    let node = ConstantNode {
        value: config.value.into(),
    };
    let metadata = mf_runtime::NodeMetadata {
        output_derivations: vec![OutputDerivation::literal("value", node.value.clone())],
        ..mf_runtime::NodeMetadata::default()
    };
    mf_runtime::PreparedNode::typed_task(node, metadata)
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(constant_factory) }
}
