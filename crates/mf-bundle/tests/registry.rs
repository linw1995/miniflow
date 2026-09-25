use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistryError, Outputs,
};
use serde_json::Value;

struct ExtraNode;

impl Node for ExtraNode {
    fn execute(&self, _inputs: Inputs) -> Result<Outputs, NodeExecutionError> {
        Ok(Outputs::new())
    }
}

fn extra_factory(_config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    Ok(Box::new(ExtraNode))
}

inventory::submit! {
    NodeRegistration {
        kind: "test.extra",
        inputs: &[],
        outputs: &[],
        factory: extra_factory,
    }
}

#[test]
fn rejects_registrations_outside_the_selected_bundle() {
    let error = mf_bundle::registry().unwrap_err();
    assert!(matches!(error, NodeRegistryError::UnexpectedKind { kind } if kind == "test.extra"));
}
