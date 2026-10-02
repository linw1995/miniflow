use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodePorts, NodeRegistration, OutputDerivation,
    Outputs, PortSpec, TaskNode, ValueType, deserialize_config,
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
    value: mf_runtime::ValueRef,
}

impl TaskNode for ConstantNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".to_owned(), self.value.clone())])).into())
    }
}
impl ConstantNode {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new(
                "value",
                ValueType::infer_shared(&self.value),
                true,
            )],
        }
    }
    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::literal("value", self.value.clone())]
    }
}

fn constant_factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: ConstantConfig = deserialize_config(config)?;
    let node = ConstantNode {
        value: config.value.into(),
    };
    let metadata = mf_runtime::NodeMetadata {
        ports: node.ports(),
        output_derivations: node.output_derivations(),
        context_references: Vec::new(),
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(constant_factory) }
}
