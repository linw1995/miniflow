use mf_runtime::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, OutputDerivation, Outputs,
    PortSpec, TaskNode, ValueType,
};
use serde_json::Value;

pub const KIND: &str = "builtin.identity";

pub fn kind() -> &'static str {
    KIND
}

struct IdentityNode;

impl TaskNode for IdentityNode {
    fn execute(
        &self,
        mut inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let Some(value) = inputs.remove("input") else {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "required input `input` was not provided".to_owned(),
            });
        };
        Ok((Outputs::from([("value".to_owned(), value)])).into())
    }
}
impl IdentityNode {
    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::forward_input("value", "input")]
    }
}

fn identity_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = IdentityNode;
    let metadata = mf_runtime::NodeMetadata {
        output_derivations: node.output_derivations(),
        ..mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Any, true)],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        })
    };
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(identity_factory) }
}
