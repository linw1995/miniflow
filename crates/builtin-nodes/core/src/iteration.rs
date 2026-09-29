use mf_runtime::{
    Inputs, IterationConfig, Node, NodeBuildError, NodeExecutionError, NodeRegistration, Outputs,
    PortSpec, ValueType, deserialize_config,
};
use serde_json::Value;

pub const KIND: &str = mf_runtime::ITERATION_KIND;

// The compiler replaces this declaration with a prepared subgraph executor after kind resolution.
struct IterationDeclaration;

impl Node for IterationDeclaration {
    fn execute(&self, _: Inputs) -> Result<Outputs, NodeExecutionError> {
        Err(NodeExecutionError::ExecutionFailed {
            message: "iteration requires a compiled body".into(),
        })
    }
}

fn factory(config: Value) -> Result<Box<dyn Node>, NodeBuildError> {
    let _: IterationConfig = deserialize_config(config)?;
    Ok(Box::new(IterationDeclaration))
}

inventory::submit! {
    NodeRegistration {
        kind: KIND,
        inputs: &[PortSpec::new("items", ValueType::Array, true)],
        outputs: &[PortSpec::new("results", ValueType::Array, true)],
        factory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::NodeRegistry;
    use serde_json::json;

    #[test]
    fn registers_iteration_and_checks_its_configuration_shape() {
        let registration = NodeRegistry::from_inventory().unwrap().get(KIND).unwrap();
        assert!(registration.instantiate(json!({})).is_err());
        let node = registration
            .instantiate(json!({
                "body": {
                    "nodes": [],
                    "result": {"node": "@iteration", "port": "items"}
                }
            }))
            .unwrap();
        let ports = registration.effective_ports(node.as_ref());
        assert_eq!(ports.inputs[0].name, "items");
        assert_eq!(ports.outputs[0].name, "results");
    }
}
