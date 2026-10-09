mod batch;
mod constant;
mod identity;
mod if_else;
mod iteration;
mod loop_declaration;
mod readline;

pub use batch::KIND as BATCH_KIND;
pub use constant::{KIND as CONSTANT_KIND, kind as constant_kind};
pub use identity::{
    IdentityInputs, IdentityOutputs, KIND as IDENTITY_KIND, kind as identity_kind, prepare_identity,
};
pub use if_else::KIND as IF_ELSE_KIND;
pub use iteration::{
    IterationInputs, IterationNode, IterationOutputs, KIND as ITERATION_KIND, MAX_PARALLEL_ITEMS,
};
pub use loop_declaration::KIND as LOOP_KIND;

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{Inputs, NodeRegistry, OutputDerivation, ValueType};
    use serde_json::json;

    #[test]
    fn one_package_preserves_constant_and_identity_values() {
        let registry = NodeRegistry::from_inventory().unwrap();
        for value in [json!(null), json!({"nested": [true, 42, "text"]})] {
            let constant = registry
                .get(CONSTANT_KIND)
                .unwrap()
                .instantiate(json!({"value": value}))
                .unwrap();
            let identity = registry
                .get(IDENTITY_KIND)
                .unwrap()
                .instantiate(json!({}))
                .unwrap();
            let produced = constant
                .execution
                .as_task_node()
                .expect("expected task execution")
                .execute(Inputs::new(), &mut mf_runtime::ExecutionContext::default())
                .unwrap()
                .outputs;
            let result = identity
                .execution
                .as_task_node()
                .expect("expected task execution")
                .execute(
                    Inputs::from([("input".into(), produced["value"].clone())]),
                    &mut mf_runtime::ExecutionContext::default(),
                )
                .unwrap()
                .outputs;
            assert_eq!(result["value"], value);
            assert!(result["value"].ptr_eq(&produced["value"]));
        }
    }

    #[test]
    fn core_nodes_expose_literal_and_forwarding_evidence() {
        let registry = NodeRegistry::from_inventory().unwrap();
        for (value, expected) in [
            (json!(42), ValueType::Int64),
            (json!(null), ValueType::Null),
            (
                json!([{"count": 1}, {"count": 2}]),
                ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64)))),
            ),
        ] {
            let registration = registry.get(CONSTANT_KIND).unwrap();
            let node = registration.instantiate(json!({"value": value})).unwrap();
            let ports = &node.metadata.ports;
            assert_eq!(ports.outputs[0].value_type, expected);
            assert_eq!(
                node.metadata.output_derivations,
                vec![OutputDerivation::literal("value", value)]
            );
            ports
                .validate_derivations("constant", &node.metadata.output_derivations)
                .unwrap();
        }

        let registration = registry.get(IDENTITY_KIND).unwrap();
        let node = registration.instantiate(json!({})).unwrap();
        let ports = &node.metadata.ports;
        assert_eq!(ports.inputs.len(), 1);
        assert_eq!(ports.inputs[0].name, "input");
        assert_eq!(ports.inputs[0].value_type, ValueType::Any);
        assert!(ports.inputs[0].required);
        assert_eq!(ports.outputs[0].value_type, ValueType::Any);
        assert_eq!(
            node.metadata.output_derivations,
            vec![OutputDerivation::forward_input("value", "input")]
        );
        ports
            .validate_derivations("identity", &node.metadata.output_derivations)
            .unwrap();
    }
}
