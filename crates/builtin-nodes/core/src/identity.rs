use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, OutputDerivation, Outputs,
    PortSpec, ValueType,
};
use serde_json::Value;

pub const KIND: &str = "builtin.identity";

pub fn kind() -> &'static str {
    KIND
}

struct IdentityNode;

impl Node for IdentityNode {
    fn context_references_complete(&self) -> bool {
        true
    }

    fn execute(&self, mut inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        let Some(value) = inputs.remove("input") else {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "required input `input` was not provided".to_owned(),
            });
        };
        Ok(Outputs::from([("value".to_owned(), value)]))
    }

    fn output_derivations(&self) -> Vec<OutputDerivation> {
        vec![OutputDerivation::forward_input("value", "input")]
    }
}

fn identity_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(IdentityNode))
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[PortSpec::new("input", ValueType::Any, true)],
        outputs: &[PortSpec::new("value", ValueType::Any, true)],
        factory: identity_factory,
    }
}
