pub mod constant;
pub mod identity;

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{Inputs, NodeRegistry};
    use serde_json::json;

    #[test]
    fn one_package_preserves_constant_and_identity_values() {
        let registry = NodeRegistry::from_inventory().unwrap();
        for value in [json!(null), json!({"nested": [true, 42, "text"]})] {
            let constant = registry
                .get(constant::KIND)
                .unwrap()
                .instantiate(json!({"value": value}))
                .unwrap();
            let identity = registry
                .get(identity::KIND)
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
}
