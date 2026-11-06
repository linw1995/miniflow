use mf_runtime::{
    NodeBuildError, NodeConfig, NodeDefinition, NodeFactory, NodeRegistration, Outputs,
    PreparedNode, PreparedSubgraph, WorkflowDefinition,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;

type Extractor = fn(&Value) -> BTreeMap<String, Value>;

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

fn extract(config: &Value, names: &[&str]) -> BTreeMap<String, Value> {
    names
        .iter()
        .filter_map(|&name| {
            config
                .get(name)
                .filter(|value| !value.is_null())
                .map(|value| (name.into(), value.clone()))
        })
        .collect()
}

fn selected(config: &Value) -> BTreeMap<String, Value> {
    extract(config, &["selected"])
}

fn registration(flow_parameters: fn(&Value) -> BTreeMap<String, Value>) -> NodeRegistration {
    NodeRegistration {
        kind: "user.dispatch",
        factory: NodeFactory::Subgraph {
            flow_parameters,
            plain: factory,
            prepared: |_, _, _| factory(Value::Null),
        },
    }
}

#[derive(NodeConfig)]
#[serde(deny_unknown_fields)]
struct DispatchConfig {
    selected: PreparedSubgraph,
    option: u32,
}

impl mf_runtime::NodePortContract for DispatchConfig {
    fn ports(&self) -> mf_runtime::NodePorts {
        mf_runtime::NodePorts {
            inputs: Vec::new(),
            outputs: self.selected.outputs.clone(),
        }
    }
}

impl mf_runtime::TaskNode for DispatchConfig {
    fn execute(
        &self,
        _: mf_runtime::Inputs,
        context: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        Ok(self.selected.execute_in_context(context)?.into())
    }
}

fn dispatch_registration() -> NodeRegistration {
    NodeRegistration {
        kind: "user.dispatch",
        factory: NodeFactory::Subgraph {
            flow_parameters: DispatchConfig::flow_parameters,
            plain: factory,
            prepared: |_, config, arguments| {
                let config = DispatchConfig::from_config(config, arguments)?;
                if config.option != 42 {
                    return mf_runtime::NodeInvalidSubgraphSnafu {
                        message: "provider requires option 42",
                    }
                    .fail();
                }
                PreparedNode::new(config, mf_runtime::NodePorts::default())
            },
        },
    }
}

fn actual_body() -> PreparedSubgraph {
    PreparedSubgraph::new(
        Vec::new(),
        vec![mf_runtime::PortSpec::new(
            "actual",
            mf_runtime::ValueType::String,
            true,
        )],
        |_| Ok(Outputs::from([("actual".into(), "prepared child".into())])),
    )
}

fn preparation_error(
    registration: NodeRegistration,
    config: Value,
    keys: &[&str],
) -> NodeBuildError {
    registration
        .instantiate_subgraphs(
            "custom",
            config,
            keys.iter().map(|key| ((*key).into(), body())).collect(),
        )
        .err()
        .unwrap()
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
        version: mf_runtime::WorkflowManifestVersion::V2026_11_06,
        graph: mf_runtime::CompiledGraph {
            definition,
            execution_order: vec!["custom".into()],
            children: BTreeMap::new(),
        },
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
    for (name, value) in [("", frozen()), (" ", frozen())] {
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
    // Duplicate keys cannot be represented by a map; derive tests reject duplicate wire names.
    let invalid_extractors: [Extractor; 2] = [
        |_| [("".into(), frozen())].into(),
        |_| [(" ".into(), frozen())].into(),
    ];
    for extractor in invalid_extractors {
        for config in [json!({}), json!({"selected": null})] {
            assert!(matches!(
                registration(extractor).validate_flow_parameters(&node(config.clone())),
                Err(NodeBuildError::FlowParameters {
                    source: mf_runtime::FlowParameterError::Invalid { .. }
                })
            ));
            for keys in [&[][..], &[""], &[" "]] {
                assert!(matches!(
                    preparation_error(registration(extractor), config.clone(), keys),
                    NodeBuildError::FlowParameters {
                        source: mf_runtime::FlowParameterError::Invalid { .. }
                    }
                ));
            }
        }
    }
    let opaque = registration(|_| BTreeMap::new());
    let malformed = node(json!({"selected": {"version": "opaque"}}));
    assert!(opaque.flow_parameters(&malformed.config).is_empty());
    assert!(
        opaque
            .validate_flow_parameters(&malformed)
            .unwrap()
            .is_empty()
    );
    let registration = registration(selected);
    for config in [json!({"selected": frozen()}), json!({"unknown": frozen()})] {
        registration
            .validate_flow_parameters(&node(config))
            .unwrap();
    }
    let plain = NodeRegistration {
        kind: "user.plain",
        factory: NodeFactory::Plain(factory),
    };
    assert!(
        plain
            .flow_parameters(&json!({"selected": frozen()}))
            .is_empty()
    );
    plain
        .validate_flow_parameters(&node(json!({"selected": frozen()})))
        .unwrap();
    let error = preparation_error(registration, json!({"unknown": frozen()}), &["unknown"]);
    assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
}

#[test]
fn validation_parses_the_receiver_callback_values_not_same_named_raw_json() {
    let registration = registration(|config| {
        BTreeMap::from([("selected".into(), config["provider"]["normalized"].clone())])
    });
    let node = node(json!({
        "selected": {"version": "opaque provider data"},
        "provider": {"normalized": frozen()},
    }));
    let original = node.clone();
    assert_eq!(
        registration.validate_flow_parameters(&node).unwrap(),
        BTreeMap::from([("selected".into(), definition())])
    );
    assert!(node.flow_parameters(&["selected"]).is_err());
    assert_eq!(node, original);
}

#[test]
fn prepared_argument_keys_must_exactly_match_frozen_config_fields() {
    let registration = registration(|config| extract(config, &["selected", "other"]));
    for value in [frozen(), json!({"path": "unresolved-child.json"})] {
        for keys in [&[][..], &["other"], &["selected", "other"]] {
            let error = preparation_error(registration, json!({"selected": value.clone()}), keys);
            assert!(matches!(error, NodeBuildError::InvalidSubgraph { message }
                if message.contains("do not match")));
        }
    }
}

#[test]
fn authoring_preserves_typed_errors_but_preparation_executes_the_supplied_child() {
    let registration = dispatch_registration();
    for value in [
        json!({"version": "2099-01-01", "dependencies": {}, "nodes": []}),
        json!({"definition": definition()}),
        json!({"path": "unresolved-child.json"}),
        json!("unresolved-child"),
    ] {
        let node = node(json!({"selected": value, "option": 42}));
        let original = node.clone();
        let error = registration.validate_flow_parameters(&node).unwrap_err();
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

        let prepared = registration
            .instantiate_subgraphs(
                "custom",
                node.config.clone(),
                BTreeMap::from([("selected".into(), actual_body())]),
            )
            .unwrap();
        let result = prepared
            .execution
            .as_task_node()
            .unwrap()
            .execute(Default::default(), &mut Default::default())
            .unwrap();
        assert_eq!(
            result.outputs,
            Outputs::from([("actual".into(), "prepared child".into())])
        );
        assert_eq!(node, original);
    }
}

#[test]
fn prepared_children_do_not_bypass_ordinary_decoding_or_provider_business_rules() {
    let registration = dispatch_registration();
    for config in [
        json!({"selected": {"path": "child.json"}}),
        json!({"selected": {"path": "child.json"}, "option": "not an integer"}),
        json!({"selected": {"path": "child.json"}, "option": 42, "unexpected": true}),
    ] {
        assert!(matches!(
            preparation_error(registration, config, &["selected"]),
            NodeBuildError::InvalidConfiguration { source } if source.is_data()
        ));
    }
    assert!(matches!(
        preparation_error(registration, json!({"selected": {"path": "child.json"}, "option": 7}), &["selected"]),
        NodeBuildError::InvalidSubgraph { message } if message == "provider requires option 42"
    ));
}

#[test]
fn plain_factories_reject_prepared_bindings_before_reading_configuration() {
    let registration = NodeRegistration {
        kind: "user.plain",
        factory: NodeFactory::Plain(factory),
    };
    for config in [Value::Null, json!({"selected": "provider-owned"})] {
        let error = preparation_error(registration, config, &["selected"]);
        assert!(matches!(&error, NodeBuildError::InvalidSubgraph { message }
            if message == "node `user.plain` does not accept prepared Flow arguments"));
        assert!(error.source().is_none());
    }
}

#[test]
fn matching_arguments_leave_nonobject_and_business_validation_to_the_provider() {
    let registration = NodeRegistration {
        kind: "user.dispatch",
        factory: NodeFactory::Subgraph {
            flow_parameters: selected,
            plain: factory,
            prepared: |id, config, arguments| {
                assert_eq!(id, "custom");
                let message = if config.is_null() {
                    assert!(arguments.is_empty());
                    "dispatch configuration must be an object"
                } else {
                    assert_eq!(config["option"], 42);
                    assert_eq!(
                        arguments.keys().map(String::as_str).collect::<Vec<_>>(),
                        ["selected"]
                    );
                    "provider business rule"
                };
                mf_runtime::NodeInvalidSubgraphSnafu { message }.fail()
            },
        },
    };
    for (config, keys, expected) in [
        (
            Value::Null,
            &[][..],
            "dispatch configuration must be an object",
        ),
        (
            json!({"selected": frozen(), "option": 42}),
            &["selected"],
            "provider business rule",
        ),
    ] {
        registration
            .validate_flow_parameters(&node(config.clone()))
            .unwrap();
        assert!(matches!(preparation_error(registration, config, keys),
            NodeBuildError::InvalidSubgraph { message } if message == expected));
    }
}
