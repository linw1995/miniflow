use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeInputs, NodeRegistration, OutputDerivation, Outputs,
    PortSpec, TypedTaskNode, ValueRef, ValueType,
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

impl TypedTaskNode for IdentityNode {
    type Input = IdentityInputs;

    fn execute(
        &self,
        inputs: IdentityInputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".to_owned(), inputs.input)])).into())
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
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        })
    };
    mf_runtime::PreparedNode::typed_task(node, metadata)
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(identity_factory) }
}
