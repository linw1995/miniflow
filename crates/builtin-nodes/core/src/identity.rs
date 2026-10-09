use mf_runtime::{
    NodeBuildError, NodeExecutionError, NodeRegistration, NodeValue, OutputDerivation,
    TypedConstructor, TypedTaskHandle, TypedTaskNode, ValueRef,
};
use serde_json::Value;

pub const KIND: &str = "builtin.identity";

pub fn kind() -> &'static str {
    KIND
}

struct IdentityNode;

#[derive(NodeValue)]
#[value(typed)]
pub struct IdentityInputs {
    input: ValueRef,
}

#[derive(NodeValue)]
#[value(typed)]
pub struct IdentityOutputs {
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

/// Constructs one Identity instance shared by typed and dynamic execution.
pub fn prepare_identity(
    _config: Value,
) -> Result<
    TypedTaskHandle<impl TypedTaskNode<Input = IdentityInputs, Output = IdentityOutputs>>,
    NodeBuildError,
> {
    let metadata = mf_runtime::NodeMetadata {
        output_derivations: vec![OutputDerivation::forward_input("value", "input")],
        ..mf_runtime::NodeMetadata::default()
    };
    TypedTaskHandle::new(
        IdentityNode,
        metadata,
        TypedConstructor {
            package: env!("CARGO_PKG_NAME"),
            path: &["prepare_identity"],
        },
        true,
    )
}

inventory::submit! {
    NodeRegistration { kind: KIND, factory: mf_runtime::NodeFactory::Plain(|config| {
        Ok(prepare_identity(config)?.prepared())
    }) }
}
