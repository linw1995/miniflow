use mf_runtime::{
    Inputs, Node, NodeBuildError, NodeExecutionError, NodeRegistration, Outputs, deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;

pub const KIND: &str = mf_runtime::LOOP_KIND;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {}

struct LoopDeclaration;

impl Node for LoopDeclaration {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "Loop requires a compiled body".into(),
        })
    }
}

fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let _: Config = deserialize_config(config)?;
    Ok(Box::new(LoopDeclaration))
}

// Loop ports depend on the typed definition and are resolved by the compiler.
inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[],
        outputs: &[],
        factory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::NodeRegistry;
    use serde_json::json;

    #[test]
    fn registers_loop_declaration_and_requires_an_empty_config() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        assert!(
            registration
                .instantiate(json!({"unexpected": true}))
                .is_err()
        );
        assert!(registration.instantiate(json!({})).is_ok());
    }
}
