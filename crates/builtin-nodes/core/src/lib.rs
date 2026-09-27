mod constant;
mod identity;
mod if_else;
mod number;

pub use constant::{KIND as CONSTANT_KIND, kind as constant_kind};
pub use identity::{KIND as IDENTITY_KIND, kind as identity_kind};
pub use if_else::KIND as IF_ELSE_KIND;

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
            let produced = constant.execute(Inputs::new()).unwrap();
            let result = identity
                .execute(Inputs::from([("input".into(), produced["value"].clone())]))
                .unwrap();
            assert_eq!(result["value"], value);
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
            let ports = registration.effective_ports(node.as_ref());
            assert_eq!(ports.outputs[0].value_type, expected);
            assert_eq!(
                node.output_derivations(),
                vec![OutputDerivation::literal("value", value)]
            );
            ports
                .validate_derivations("constant", &node.output_derivations())
                .unwrap();
        }

        let registration = registry.get(IDENTITY_KIND).unwrap();
        let node = registration.instantiate(json!({})).unwrap();
        let ports = registration.effective_ports(node.as_ref());
        assert_eq!(ports.outputs[0].value_type, ValueType::Any);
        assert_eq!(
            node.output_derivations(),
            vec![OutputDerivation::forward_input("value", "input")]
        );
        ports
            .validate_derivations("identity", &node.output_derivations())
            .unwrap();
    }
}
