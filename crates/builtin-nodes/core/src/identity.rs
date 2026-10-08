use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeInputs, NodeOutputs, NodeRegistration,
    OutputDerivation, TypedTaskNode, ValueRef,
};
use serde_json::Value;

pub const KIND: &str = "builtin.identity";

pub fn kind() -> &'static str {
    KIND
}

struct IdentityNode;

#[derive(NodeInputs)]
struct IdentityInputs {
    input: ValueRef,
}

#[derive(NodeOutputs)]
struct IdentityOutputs {
    value: ValueRef,
}

impl TypedTaskNode for IdentityNode {
    type Input = IdentityInputs;
    type Output = IdentityOutputs;

    fn execute(
        &self,
        inputs: IdentityInputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::TypedNodeResult<Self::Output>, NodeExecutionError> {
        Ok(IdentityOutputs {
            value: inputs.input,
        }
        .into())
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
        ..mf_runtime::NodeMetadata::default()
    };
    mf_runtime::PreparedNode::typed_task(node, metadata)
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(identity_factory) }
}
