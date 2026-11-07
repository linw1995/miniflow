extern crate mf_runtime as runtime_alias;

use mf_runtime::{Flow, NodeConfig, PreparedSubgraph};
use serde_json::{Value as JsonValue, json};
use std::collections::BTreeMap;

#[derive(NodeConfig)]
#[serde(rename_all = "camelCase")]
struct DirectConfig {
    request_body: Flow,
    #[serde(rename = "left-hand", default)]
    left: Option<Flow>,
    right: ::std::option::Option<::mf_runtime::Flow>,
    #[serde(alias = "preferences")]
    settings: Option<serde_json::Value>,
}

#[derive(NodeConfig)]
#[config(runtime = "::runtime_alias")]
struct AliasedRuntimeConfig {
    body: runtime_alias::Flow,
    left: ::std::option::Option<::runtime_alias::Flow>,
}

fn prepared() -> PreparedSubgraph {
    PreparedSubgraph::from_flow(
        Flow::from_plan(
            vec![],
            mf_runtime::FlowPlan {
                connections: vec![].into(),
                dependencies: vec![].into(),
                execution_order: vec![].into(),
                outputs: vec![].into(),
                execution_domains: mf_runtime::ExecutionDomains::from_parts(
                    vec![].into(),
                    vec![].into(),
                ),
            },
            Default::default(),
        ),
        vec![],
    )
}

#[test]
fn actual_derive_injects_compiled_flows_and_preserves_ordinary_serde_fields() {
    assert_eq!(
        DirectConfig::flow_parameters(&json!({
            "requestBody": {"request": true},
            "left-hand": {"left": true},
            "right": null,
            "settings": {"body": {}},
            "unknown": {"nodes": []},
        }))
        .unwrap(),
        BTreeMap::from([
            ("/requestBody".into(), json!({"request": true})),
            ("/left-hand".into(), json!({"left": true})),
            ("/right".into(), JsonValue::Null),
        ])
    );
    let config = DirectConfig::from_config(
        json!({
            "requestBody": {"source": "already prepared"},
            "left-hand": {"source": "already prepared"},
            "preferences": {"ordinary": true},
        }),
        BTreeMap::from([
            ("/requestBody".into(), prepared()),
            ("/left-hand".into(), prepared()),
        ]),
    )
    .unwrap();
    let cloned = config.request_body.clone();
    assert!(config.request_body.execute().unwrap().is_empty());
    assert!(cloned.execute().unwrap().is_empty());
    assert!(config.left.unwrap().execute().unwrap().is_empty());
    assert!(config.right.is_none());
    assert_eq!(config.settings, Some(json!({"ordinary": true})));
}

#[test]
fn actual_derive_accepts_qualified_types_with_a_runtime_crate_alias() {
    assert_eq!(
        AliasedRuntimeConfig::flow_parameters(&json!({"body": {}, "left": null})).unwrap(),
        BTreeMap::from([
            ("/body".into(), json!({})),
            ("/left".into(), JsonValue::Null)
        ])
    );
    let config = AliasedRuntimeConfig::from_config(
        json!({"body": {}, "left": null}),
        BTreeMap::from([("/body".into(), prepared())]),
    )
    .unwrap();
    assert!(config.body.execute().unwrap().is_empty());
    assert!(config.left.is_none());
}

#[test]
fn callback_config_receives_its_typed_prepared_body_without_a_fake_flow() {
    #[derive(NodeConfig)]
    struct CallbackConfig {
        body: PreparedSubgraph,
    }
    let callback = || PreparedSubgraph::new(vec![], vec![], |_| Ok(Default::default()));
    let config = CallbackConfig::from_config(
        json!({"body": {}}),
        BTreeMap::from([("/body".into(), callback())]),
    )
    .unwrap();
    assert!(
        config
            .body
            .execute_in_context(&mut Default::default())
            .unwrap()
            .is_empty()
    );
    let result = AliasedRuntimeConfig::from_config(
        json!({"body": {}}),
        BTreeMap::from([("/body".into(), callback())]),
    );
    assert!(
        matches!(result, Err(mf_runtime::NodeBuildError::InvalidSubgraph { message })
        if message.contains("not backed by a compiled finite Flow"))
    );
}

#[test]
fn ordinary_serde_paths_keep_the_original_self_receiver() {
    #[derive(NodeConfig)]
    struct Config {
        body: Option<Flow>,
        #[serde(
            default = "Self::default_count",
            deserialize_with = "Self::decode_count"
        )]
        count: usize,
    }
    impl Config {
        fn default_count() -> usize {
            7
        }

        fn decode_count<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<usize, D::Error> {
            <usize as serde::Deserialize>::deserialize(decoder).map(|count| count + 1)
        }
    }
    let defaulted = Config::from_config(json!({}), BTreeMap::new()).unwrap();
    assert_eq!(defaulted.count, 7);
    assert!(defaulted.body.is_none());
    let decoded = Config::from_config(
        json!({"count": 4, "body": {}}),
        BTreeMap::from([("/body".into(), prepared())]),
    )
    .unwrap();
    assert_eq!(decoded.count, 5);
    assert!(decoded.body.unwrap().execute().unwrap().is_empty());
}

#[test]
fn ordinary_helper_names_do_not_shadow_user_types_or_container_defaults() {
    #[derive(serde::Deserialize)]
    struct OrdinaryConfig {
        count: usize,
    }
    #[derive(NodeConfig)]
    struct Config {
        ordinary: OrdinaryConfig,
        body: Option<Flow>,
    }
    let config = Config::from_config(json!({"ordinary": {"count": 11}}), BTreeMap::new()).unwrap();
    assert_eq!(config.ordinary.count, 11);
    assert!(config.body.is_none());

    {
        #[derive(NodeConfig)]
        #[serde(default)]
        struct OrdinaryConfig {
            count: usize,
            body: Option<Flow>,
        }
        impl Default for OrdinaryConfig {
            fn default() -> Self {
                Self {
                    count: 17,
                    body: None,
                }
            }
        }
        let config = OrdinaryConfig::from_config(
            json!({"body": {}}),
            BTreeMap::from([("/body".into(), prepared())]),
        )
        .unwrap();
        assert_eq!(config.count, 17);
        assert!(config.body.unwrap().execute().unwrap().is_empty());
    }
}

#[test]
#[allow(non_snake_case)]
fn parameter_names_match_serde_deserialization_for_every_rename_rule() {
    macro_rules! assert_parity {
        ($rule:tt, $settings_name:literal) => {{
            // Keep identical fields but independent derives: Serde is the oracle
            // even for the executable placeholders removed by NodeConfig.
            macro_rules! config {
                ($name:ident, $derive:meta, $flow:ident) => {
                    #[$derive]
                    #[serde(rename_all = $rule, deny_unknown_fields)]
                    struct $name {
                        request_body: $flow,
                        optional_body: Option<$flow>,
                        r#type: $flow,
                        ordinary_field: usize,
                        mixedCase: usize,
                        r#match: usize,
                        #[serde(rename(serialize = "output-only", deserialize = "input-only"))]
                        directional_field: usize,
                        #[serde(alias = "legacy-preferences")]
                        user_settings: usize,
                        #[serde(rename = "explicit-body")]
                        __: $flow,
                    }
                };
            }
            config!(Config, derive(NodeConfig), Flow);
            config!(SerdeConfig, derive(serde::Serialize, serde::Deserialize), JsonValue);

            // Construct canonical input independently of the receiver callback.
            // Serde's serializer provides the names; the directional ordinary
            // field deliberately uses its deserialize name on the input wire.
            let mut canonical = serde_json::to_value(SerdeConfig {
                request_body: json!({"request": true}),
                optional_body: Some(json!({"optional": true})),
                r#type: json!({"type": true}),
                ordinary_field: 17,
                mixedCase: 17,
                r#match: 17,
                directional_field: 17,
                user_settings: 17,
                __: json!({"explicit": true}),
            }).unwrap();
            let fields = canonical.as_object_mut().unwrap();
            let directional = fields.remove("output-only").unwrap();
            fields.insert("input-only".into(), directional);
            let expected = fields.iter()
                .filter(|(_, value)| value.is_object())
                .map(|(name, value)| (mf_runtime::node_config::join_config_path("", name), value.clone()))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(expected.len(), 4, "{}", $rule);
            assert_eq!(Config::flow_parameters(&canonical).unwrap(), expected);
            let mut aliased = canonical.clone();
            let fields = aliased.as_object_mut().unwrap();
            let settings = fields.remove($settings_name).unwrap();
            fields.insert("legacy-preferences".into(), settings);

            for input in [canonical, aliased] {
                let decoded: SerdeConfig = serde_json::from_value(input.clone()).unwrap();
                assert_eq!(
                    [
                        decoded.request_body,
                        decoded.optional_body.unwrap(),
                        decoded.r#type,
                        decoded.__
                    ],
                    [
                        json!({"request": true}), json!({"optional": true}),
                        json!({"type": true}), json!({"explicit": true}),
                    ]
                );
                let ordinary = [
                    decoded.ordinary_field,
                    decoded.mixedCase,
                    decoded.r#match,
                    decoded.directional_field,
                    decoded.user_settings,
                ];
                assert_eq!(ordinary, [17; 5]);

                let extracted = Config::flow_parameters(&input).unwrap();
                assert_eq!(extracted, expected);
                assert!(!extracted.contains_key($settings_name));
                assert!(!extracted.contains_key("input-only"));
                assert!(!extracted.contains_key("legacy-preferences"));
                let flows = extracted.into_keys().map(|name| (name, prepared())).collect();
                let config = Config::from_config(input, flows).unwrap();
                for flow in [
                    config.request_body,
                    config.optional_body.unwrap(),
                    config.r#type,
                    config.__,
                ] {
                    assert!(flow.execute().unwrap().is_empty());
                }
                assert_eq!(
                    [
                        config.ordinary_field,
                        config.mixedCase,
                        config.r#match,
                        config.directional_field,
                        config.user_settings
                    ],
                    ordinary
                );
            }
        }};
    }

    assert_parity!("lowercase", "user_settings");
    assert_parity!("UPPERCASE", "USER_SETTINGS");
    assert_parity!("PascalCase", "UserSettings");
    assert_parity!("camelCase", "userSettings");
    assert_parity!("snake_case", "user_settings");
    assert_parity!("SCREAMING_SNAKE_CASE", "USER_SETTINGS");
    assert_parity!("kebab-case", "user-settings");
    assert_parity!("SCREAMING-KEBAB-CASE", "USER-SETTINGS");
}

#[test]
fn extraction_selects_all_executable_field_forms_without_decoding_provider_data() {
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Config {
        required: Flow,
        optional: Option<Flow>,
        callback: PreparedSubgraph,
        optional_callback: Option<PreparedSubgraph>,
        ordinary: Option<JsonValue>,
    }
    let input = json!({
        "required": false,
        "optional": 0,
        "callback": ["not decoded here"],
        "optional_callback": "source.json",
        "ordinary": {"version": "2026-10-28", "dependencies": {}, "nodes": []},
        "unknown": {"body": {}},
    });
    let original = input.clone();
    assert_eq!(
        Config::flow_parameters(&input).unwrap(),
        BTreeMap::from([
            ("/required".into(), json!(false)),
            ("/optional".into(), json!(0)),
            ("/callback".into(), json!(["not decoded here"])),
            ("/optional_callback".into(), json!("source.json")),
        ])
    );
    assert_eq!(input, original);
    for input in [
        JsonValue::Null,
        json!(false),
        json!(42),
        json!("opaque"),
        json!([]),
    ] {
        assert!(Config::flow_parameters(&input).is_err());
    }
    for input in [
        json!({}),
        json!({"required": null, "optional": null, "callback": null, "optional_callback": null}),
    ] {
        assert_eq!(
            Config::flow_parameters(&input).unwrap(),
            BTreeMap::from([
                ("/required".into(), JsonValue::Null),
                ("/optional".into(), JsonValue::Null),
                ("/callback".into(), JsonValue::Null),
                ("/optional_callback".into(), JsonValue::Null),
            ])
        );
    }
}

#[test]
fn ordinary_missing_fields_remain_distinct_from_explicit_null() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CODECS: AtomicUsize = AtomicUsize::new(0);
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Required {
        body: Option<Flow>,
        payload: JsonValue,
    }
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Optional {
        body: Option<Flow>,
        payload: Option<JsonValue>,
    }
    struct Nullable(JsonValue);
    impl<'de> serde::Deserialize<'de> for Nullable {
        fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            CODECS.fetch_add(1, Ordering::SeqCst);
            <JsonValue as serde::Deserialize>::deserialize(decoder).map(Self)
        }
    }
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Custom {
        body: Option<Flow>,
        payload: Nullable,
    }
    for input in [json!({}), json!({"payload": null})] {
        let before = CODECS.load(Ordering::SeqCst);
        let roles = BTreeMap::from([("/body".into(), JsonValue::Null)]);
        assert_eq!(Required::flow_parameters(&input).unwrap(), roles);
        assert_eq!(Optional::flow_parameters(&input).unwrap(), roles);
        assert_eq!(Custom::flow_parameters(&input).unwrap(), roles);
        assert_eq!(CODECS.load(Ordering::SeqCst), before);
        assert!(
            Optional::from_config(input.clone(), BTreeMap::new())
                .unwrap()
                .payload
                .is_none()
        );
        if input.get("payload").is_some() {
            assert_eq!(
                Required::from_config(input.clone(), BTreeMap::new())
                    .unwrap()
                    .payload,
                JsonValue::Null
            );
            assert_eq!(
                Custom::from_config(input, BTreeMap::new())
                    .unwrap()
                    .payload
                    .0,
                JsonValue::Null
            );
        } else {
            let error = Required::from_config(input.clone(), BTreeMap::new())
                .err()
                .unwrap();
            assert!(
                matches!(&error, mf_runtime::NodeBuildError::InvalidConfiguration { source }
                if source.is_data() && source.to_string().contains("missing field"))
            );
            assert!(
                std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<serde_json::Error>()
                    .is_some()
            );
            assert!(matches!(Custom::from_config(input, BTreeMap::new()),
                Err(mf_runtime::NodeBuildError::InvalidConfiguration { source })
                if source.is_data() && source.to_string().contains("missing field")));
        }
    }
    assert_eq!(
        JsonValue::from_config(JsonValue::Null, BTreeMap::new()).unwrap(),
        JsonValue::Null
    );
}

#[test]
fn skipped_ordinary_fields_use_serde_default_priority_only_during_decode() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FIELD_DEFAULTS: AtomicUsize = AtomicUsize::new(0);
    static CONTAINER_DEFAULTS: AtomicUsize = AtomicUsize::new(0);
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct FieldDefault {
        body: Option<Flow>,
        #[serde(skip_deserializing, default = "Self::default_count")]
        count: usize,
    }
    impl FieldDefault {
        fn default_count() -> usize {
            FIELD_DEFAULTS.fetch_add(1, Ordering::SeqCst);
            7
        }
    }
    #[derive(NodeConfig)]
    #[serde(default = "Self::defaults")]
    #[allow(dead_code)]
    struct ContainerDefault {
        #[serde(skip_deserializing)]
        count: usize,
        #[serde(skip_deserializing, default)]
        field_default_wins: usize,
    }
    impl ContainerDefault {
        fn defaults() -> Self {
            CONTAINER_DEFAULTS.fetch_add(1, Ordering::SeqCst);
            Self {
                count: 9,
                field_default_wins: 11,
            }
        }
    }
    // The custom default belongs to an ordinary nested NodeConfig, not the
    // Flow-bearing parent. It therefore cannot construct executable defaults.
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct WithFlowSibling {
        body: Option<Flow>,
        ordinary: ContainerDefault,
    }
    for input in [json!({}), json!({"count": 99, "field_default_wins": 99})] {
        let fields_before = FIELD_DEFAULTS.load(Ordering::SeqCst);
        let containers_before = CONTAINER_DEFAULTS.load(Ordering::SeqCst);
        let roles = BTreeMap::from([("/body".into(), JsonValue::Null)]);
        let nested = json!({"ordinary": input.clone()});
        assert_eq!(FieldDefault::flow_parameters(&input).unwrap(), roles);
        assert!(
            ContainerDefault::flow_parameters(&input)
                .unwrap()
                .is_empty()
        );
        assert_eq!(WithFlowSibling::flow_parameters(&nested).unwrap(), roles);
        assert_eq!(FIELD_DEFAULTS.load(Ordering::SeqCst), fields_before);
        assert_eq!(CONTAINER_DEFAULTS.load(Ordering::SeqCst), containers_before);
        assert_eq!(
            FieldDefault::from_config(input.clone(), BTreeMap::new())
                .unwrap()
                .count,
            7
        );
        let decoded = WithFlowSibling::from_config(nested, BTreeMap::new())
            .unwrap()
            .ordinary;
        assert_eq!(decoded.count, 9);
        assert_eq!(decoded.field_default_wins, 0);
        assert_eq!(FIELD_DEFAULTS.load(Ordering::SeqCst), fields_before + 1);
        assert_eq!(
            CONTAINER_DEFAULTS.load(Ordering::SeqCst),
            containers_before + 1
        );
    }
}

#[test]
fn custom_container_defaults_on_flow_bearing_configs_are_rejected_without_execution() {
    #[derive(NodeConfig)]
    #[serde(default = "Self::defaults")]
    #[allow(dead_code)]
    struct Config {
        body: Option<Flow>,
    }
    impl Config {
        fn defaults() -> Self {
            panic!("discovery and rejected decoding must never run executable defaults")
        }
    }
    for input in [json!({}), json!({"body": null}), json!({"body": {}})] {
        assert!(matches!(Config::flow_parameters(&input),
            Err(mf_runtime::NodeBuildError::InvalidSubgraph { message })
            if message.contains("executable defaults cannot supply Flow values")));
        assert!(matches!(Config::from_config(input, BTreeMap::new()),
            Err(mf_runtime::NodeBuildError::InvalidSubgraph { message })
            if message.contains("executable defaults cannot supply Flow values")));
    }
}

#[test]
fn metadata_ignores_unknown_ordinary_fields_but_provider_decode_retains_serde_errors() {
    #[derive(NodeConfig)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Config {
        body: Flow,
    }
    let input = json!({
        "body": {"path": "already-prepared.json"},
        "unexpected": {"version": "2026-10-28", "nodes": []},
    });
    let original = input.clone();
    assert_eq!(
        Config::flow_parameters(&input).unwrap(),
        BTreeMap::from([("/body".into(), input["body"].clone()),])
    );
    assert_eq!(input, original);
    let error = Config::from_config(input, BTreeMap::from([("/body".into(), prepared())]))
        .err()
        .unwrap();
    assert!(
        matches!(&error, mf_runtime::NodeBuildError::InvalidConfiguration { source }
        if source.is_data() && source.to_string().contains("unknown field `unexpected`"))
    );
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<serde_json::Error>()
            .is_some()
    );
}

#[test]
fn flow_config_cannot_be_decoded_without_its_prepared_body() {
    assert!(AliasedRuntimeConfig::from_config(json!({"body": {}}), BTreeMap::new()).is_err());
    assert!(
        AliasedRuntimeConfig::from_config(
            json!({"body": {}}),
            BTreeMap::from([
                ("/body".into(), prepared()),
                ("/undeclared".into(), prepared())
            ]),
        )
        .is_err()
    );
}
