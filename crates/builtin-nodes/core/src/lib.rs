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
    use mf_runtime::{
        ControlEdgeDefinition, DefinitionId, EdgeDefinition, Flow, FlowNode, Inputs, NodeRegistry,
        OutputDerivation, ValueType, WorkflowOutputDefinition,
    };
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

    #[test]
    fn skipped_identity_does_not_publish_a_value() {
        let registry = NodeRegistry::from_inventory().unwrap();
        let instance = |id: &str, kind: &str, config| {
            let registration = registry.get(kind).unwrap();
            let node = registration.instantiate(config).unwrap();
            let ports = registration.effective_ports(node.as_ref());
            FlowNode::new(id, node, ports)
        };
        let nodes = vec![
            instance("data", CONSTANT_KIND, json!({"value": null})),
            instance("trigger", CONSTANT_KIND, json!({"value": false})),
            instance(
                "route",
                IF_ELSE_KIND,
                json!({"branches": [{
                    "id": "on",
                    "condition": {
                        "source": {"output": "trigger.value", "path": ""},
                        "operator": "eq",
                        "value": true
                    }
                }]}),
            ),
            instance("identity", IDENTITY_KIND, json!({})),
        ];
        let flow = Flow::new(
            nodes,
            vec![EdgeDefinition {
                from_node: DefinitionId::from("data"),
                from_output: "value".into(),
                to_node: DefinitionId::from("identity"),
                to_input: "input".into(),
            }],
            ["data", "trigger", "route", "identity"]
                .into_iter()
                .map(DefinitionId::from)
                .collect(),
            vec![WorkflowOutputDefinition {
                name: "result".into(),
                node: DefinitionId::from("identity"),
                port: "value".into(),
                optional: true,
            }],
        )
        .unwrap()
        .with_control_edges(vec![
            ControlEdgeDefinition {
                from_node: DefinitionId::from("trigger"),
                from_output: "value".into(),
                to_node: DefinitionId::from("route"),
            },
            ControlEdgeDefinition {
                from_node: DefinitionId::from("route"),
                from_output: "on".into(),
                to_node: DefinitionId::from("identity"),
            },
        ])
        .unwrap();
        assert!(flow.execute().unwrap().is_empty());
    }
}
