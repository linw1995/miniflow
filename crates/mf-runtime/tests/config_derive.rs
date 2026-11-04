extern crate mf_runtime as runtime_alias;

use mf_runtime::{ConfigParameterType, Flow, NodeConfig, PreparedSubgraph};
use serde_json::json;
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
        DirectConfig::parameter_types(),
        BTreeMap::from([
            ("requestBody".into(), ConfigParameterType::Flow),
            (
                "left-hand".into(),
                ConfigParameterType::Optional(Box::new(ConfigParameterType::Flow))
            ),
            (
                "right".into(),
                ConfigParameterType::Optional(Box::new(ConfigParameterType::Flow))
            ),
            ("settings".into(), ConfigParameterType::Value),
        ])
    );
    let config = DirectConfig::from_config(
        json!({
            "requestBody": {"source": "already prepared"},
            "left-hand": {"source": "already prepared"},
            "preferences": {"ordinary": true},
        }),
        BTreeMap::from([
            ("requestBody".into(), prepared()),
            ("left-hand".into(), prepared()),
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
        AliasedRuntimeConfig::parameter_types(),
        BTreeMap::from([
            ("body".into(), ConfigParameterType::Flow),
            (
                "left".into(),
                ConfigParameterType::Optional(Box::new(ConfigParameterType::Flow))
            ),
        ])
    );
    let config = AliasedRuntimeConfig::from_config(
        json!({"body": {}, "left": null}),
        BTreeMap::from([("body".into(), prepared())]),
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
        BTreeMap::from([("body".into(), callback())]),
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
        BTreeMap::from([("body".into(), callback())]),
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
        BTreeMap::from([("body".into(), prepared())]),
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
            BTreeMap::from([("body".into(), prepared())]),
        )
        .unwrap();
        assert_eq!(config.count, 17);
        assert!(config.body.unwrap().execute().unwrap().is_empty());
    }
}

#[test]
fn flow_config_cannot_be_decoded_without_its_prepared_body() {
    assert!(AliasedRuntimeConfig::from_config(json!({"body": {}}), BTreeMap::new()).is_err());
    assert!(
        AliasedRuntimeConfig::from_config(
            json!({"body": {}}),
            BTreeMap::from([
                ("body".into(), prepared()),
                ("undeclared".into(), prepared())
            ]),
        )
        .is_err()
    );
}
