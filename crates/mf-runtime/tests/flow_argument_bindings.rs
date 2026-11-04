use mf_runtime::{
    NodeBuildError, NodeDefinition, NodeFactory, NodeRegistration, Outputs, PreparedNode,
    PreparedSubgraph, WorkflowDefinition,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;

fn definition() -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {}, "nodes": []
    }))
    .unwrap()
}

fn frozen() -> Value {
    json!(definition())
}

fn node(config: Value) -> NodeDefinition {
    serde_json::from_value(json!({
        "id": "custom", "kind": "user.dispatch", "config": config
    }))
    .unwrap()
}

fn body() -> PreparedSubgraph {
    PreparedSubgraph::new(Vec::new(), Vec::new(), |_| Ok(Outputs::new()))
}

fn factory(_: Value) -> Result<PreparedNode, NodeBuildError> {
    panic!("validation must happen before calling the provider")
}

fn registration(parameters: &'static [&'static str]) -> NodeRegistration {
    NodeRegistration {
        kind: "user.dispatch",
        factory: NodeFactory::Subgraph {
            parameters,
            plain: factory,
            prepared: |_, _, _| factory(Value::Null),
        },
    }
}

#[test]
fn legacy_provider_config_remains_opaque() {
    let node = node(json!({"selected": frozen(), "subgraphs": "provider-owned"}));
    assert!(node.flow_parameters(&[]).unwrap().is_empty());
    assert_eq!(node.flow_parameters(&["selected"]).unwrap().len(), 1);
    let definition = serde_json::from_value(json!({
        "version": "2026-10-28", "dependencies": {}, "nodes": [node]
    }))
    .unwrap();
    let manifest = mf_runtime::WorkflowManifest {
        version: mf_runtime::WorkflowManifestVersion::V2026_10_16,
        definition,
        execution_order: vec!["custom".into()],
        loop_orders: vec![],
        input_schema: mf_runtime::WorkflowInputSchema {
            inputs: [("custom".into(), Default::default())].into(),
            ..Default::default()
        },
    };
    assert!(manifest.description().unwrap().loop_bodies.is_empty());
    assert!(manifest.to_bytes().is_ok());
}

#[test]
fn direct_definitions_round_trip_and_sources_keep_their_exact_schema() {
    let value = frozen();
    let child: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(child).unwrap(), value);
    for source in [
        json!({"body": {"nodes": []}}),
        json!({"builtin": "example", "config": {}}),
        json!({"builtin": "example", "flows": {}}),
    ] {
        assert!(serde_json::from_value::<mf_runtime::FlowSource>(source).is_err());
    }
}

#[test]
fn removed_root_flow_arguments_are_rejected_for_every_schema() {
    for version in ["2026-10-25", "2026-10-28"] {
        for arguments in [json!({}), json!({"provided": {"definition": definition()}})] {
            assert!(
                serde_json::from_value::<WorkflowDefinition>(json!({
                    "version": version, "dependencies": {}, "nodes": [],
                    "flow_arguments": arguments
                }))
                .is_err()
            );
        }
    }
}

#[test]
fn only_declared_immediate_fields_select_flow_parameters() {
    let selected_node = node(json!({
        "selected": frozen(),
        "custom_option": 42,
        "nested": {"child": frozen()},
        "ordinary_definition": {"definition": definition()},
        "subgraphs": {"else": {"parameter": "provided"}}
    }));
    assert!(selected_node.flow_parameters(&[]).unwrap().is_empty());
    assert_eq!(
        selected_node.flow_parameters(&["selected"]).unwrap(),
        BTreeMap::from([("selected".into(), definition())])
    );
    let encoded = serde_json::to_value(&selected_node).unwrap();
    let decoded: NodeDefinition = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        decoded.flow_parameters(&["selected"]).unwrap(),
        selected_node.flow_parameters(&["selected"]).unwrap()
    );
    for config in [
        json!({}),
        json!({"parameter": "provided"}),
        json!({"nested": {"child": frozen()}}),
    ] {
        let node = node(config);
        assert!(node.flow_parameters(&["selected"]).unwrap().is_empty());
    }
}

#[test]
fn malformed_declared_fields_are_rejected_but_absent_and_null_fields_are_optional() {
    for (name, value) in [
        ("", frozen()),
        (" ", frozen()),
        ("selected", json!({"$flow": definition()})),
    ] {
        assert!(node(json!({name: value})).flow_parameters(&[name]).is_err());
    }
    for config in [json!({}), json!({"selected": null})] {
        assert!(
            node(config)
                .flow_parameters(&["selected"])
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn declarations_are_validated_but_unknown_json_is_opaque() {
    for parameters in [&[" "][..], &["selected", "selected"][..]] {
        assert!(
            registration(parameters)
                .validate_flow_parameters(&node(json!({})))
                .is_err()
        );
    }
    let registration = registration(&["selected"]);
    for config in [
        json!({"selected": frozen()}),
        json!({"unknown": frozen()}),
        json!({"selected": frozen(), "unknown": {"$flow": null}}),
    ] {
        registration
            .validate_flow_parameters(&node(config))
            .unwrap();
    }
    let plain = NodeRegistration {
        kind: "user.plain",
        factory: NodeFactory::Plain(factory),
    };
    assert!(plain.flow_parameters().is_empty());
    plain
        .validate_flow_parameters(&node(json!({"selected": frozen()})))
        .unwrap();
    let error = registration
        .instantiate_subgraphs(
            "custom",
            json!({"unknown": frozen()}),
            BTreeMap::from([("unknown".into(), body())]),
        )
        .err()
        .unwrap();
    assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
}

#[test]
fn prepared_argument_keys_must_exactly_match_frozen_config_fields() {
    let registration = registration(&["selected", "other"]);
    for arguments in [
        BTreeMap::new(),
        BTreeMap::from([("other".into(), body())]),
        BTreeMap::from([("selected".into(), body()), ("other".into(), body())]),
    ] {
        let error = registration
            .instantiate_subgraphs("custom", json!({"selected": frozen()}), arguments)
            .err()
            .unwrap();
        assert!(error.to_string().contains("do not match"));
    }
}

#[test]
fn malformed_declared_frozen_fields_preserve_the_typed_error_chain() {
    let registration = registration(&["selected"]);
    for value in [
        json!({"$flow": null}),
        json!({"version": "2099-01-01", "dependencies": {}, "nodes": []}),
        json!({"$flow": definition(), "extra": true}),
    ] {
        let node = node(json!({"selected": value}));
        let original = node.clone();
        let validation_error = registration.validate_flow_parameters(&node).unwrap_err();
        let preparation_error = registration
            .instantiate_subgraphs(
                "custom",
                node.config.clone(),
                BTreeMap::from([("selected".into(), body())]),
            )
            .err()
            .unwrap();
        for error in [validation_error, preparation_error] {
            assert!(matches!(&error, NodeBuildError::FlowParameters {
                source: mf_runtime::FlowParameterError::Json { parameter, source }
            } if parameter == "selected" && source.is_data()));
            let parameters = error
                .source()
                .unwrap()
                .downcast_ref::<mf_runtime::FlowParameterError>()
                .unwrap();
            let json = parameters
                .source()
                .unwrap()
                .downcast_ref::<serde_json::Error>()
                .unwrap();
            assert!(json.is_data());
            assert!(json.source().is_none());
        }
        assert_eq!(node, original);
    }
}

#[test]
fn plain_factories_reject_prepared_bindings_before_inspecting_reserved_markers() {
    let registration = NodeRegistration {
        kind: "user.plain",
        factory: NodeFactory::Plain(factory),
    };
    for config in [Value::Null, json!({"selected": {"$flow": null}})] {
        let error = registration
            .instantiate_subgraphs(
                "custom",
                config,
                BTreeMap::from([("selected".into(), body())]),
            )
            .err()
            .unwrap();
        assert!(matches!(&error, NodeBuildError::InvalidSubgraph { message }
            if message == "node `user.plain` does not accept prepared Flow arguments"));
        assert!(error.source().is_none());
    }
}

#[test]
fn absent_frozen_fields_leave_nonobject_config_validation_to_the_provider() {
    let registration = NodeRegistration {
        kind: "user.dispatch",
        factory: NodeFactory::Subgraph {
            parameters: &["selected"],
            plain: factory,
            prepared: |id, config, arguments| {
                assert_eq!(id, "custom");
                assert!(config.is_null());
                assert!(arguments.is_empty());
                mf_runtime::NodeInvalidSubgraphSnafu {
                    message: "dispatch configuration must be an object",
                }
                .fail()
            },
        },
    };
    let node = node(Value::Null);
    registration.validate_flow_parameters(&node).unwrap();
    let error = registration
        .instantiate_subgraphs("custom", node.config, BTreeMap::new())
        .err()
        .unwrap();
    assert!(matches!(error, NodeBuildError::InvalidSubgraph { message }
        if message == "dispatch configuration must be an object"));
}

#[test]
fn matching_arguments_reach_provider_business_validation() {
    let registration = NodeRegistration {
        kind: "user.dispatch",
        factory: NodeFactory::Subgraph {
            parameters: &["selected"],
            plain: factory,
            prepared: |id, config, arguments| {
                assert_eq!(id, "custom");
                assert_eq!(config["option"], 42);
                assert!(arguments.contains_key("selected"));
                mf_runtime::NodeInvalidSubgraphSnafu {
                    message: "provider business rule",
                }
                .fail()
            },
        },
    };
    let error = registration
        .instantiate_subgraphs(
            "custom",
            json!({"selected": frozen(), "option": 42}),
            BTreeMap::from([("selected".into(), body())]),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("provider business rule"));
}
