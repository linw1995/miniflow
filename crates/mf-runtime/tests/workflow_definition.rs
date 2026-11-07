use mf_runtime::{DefinitionParseError, WorkflowDefinition, WorkflowDefinitionVersion};
use serde_json::{Value, json};

#[test]
fn empty_dependencies_are_optional_and_canonical_but_null_is_invalid() {
    for dependencies in [None, Some(json!({}))] {
        let mut value = json!({"version":"2026-10-28", "nodes":[]});
        if let Some(dependencies) = dependencies {
            value["dependencies"] = dependencies;
        }
        let definition = WorkflowDefinition::from_json(&value.to_string()).unwrap();
        assert!(definition.dependencies.is_empty());
        let canonical = serde_json::to_value(&definition).unwrap();
        assert!(canonical.get("dependencies").is_none());
        assert_eq!(
            serde_json::from_value::<WorkflowDefinition>(canonical).unwrap(),
            definition
        );
    }
    assert!(
        WorkflowDefinition::from_json(r#"{"version":"2026-10-28","dependencies":null,"nodes":[]}"#)
            .is_err()
    );
}

#[test]
fn only_an_authorized_current_scope_inherits_a_missing_version() {
    let opaque = json!({"nodes":[], "body":{"path":"must-not-read.json"}});
    let value = json!({"nodes":[{"id":"owner", "kind":"unknown.receiver", "config":{
        "body":opaque, "other":{"definition":{"nodes":[]}}
    }}]});
    assert!(serde_json::from_value::<WorkflowDefinition>(value.clone()).is_err());
    assert!(WorkflowDefinition::from_json(&value.to_string()).is_err());
    let parsed =
        WorkflowDefinition::from_value_with_version(value, WorkflowDefinitionVersion::V2026_10_03)
            .unwrap();
    assert_eq!(parsed.version, WorkflowDefinitionVersion::V2026_10_03);
    assert_eq!(parsed.nodes[0].config["body"], opaque);
    assert_eq!(
        parsed.nodes[0].config["other"],
        json!({"definition":{"nodes":[]}})
    );
    let canonical = serde_json::to_value(parsed).unwrap();
    assert_eq!(canonical["version"], "2026-10-03");
    assert!(
        canonical["nodes"][0]["config"]["body"]
            .get("version")
            .is_none()
    );
}

#[test]
fn explicit_child_versions_are_never_defaulted_or_overridden() {
    for version in [Value::Null, json!(42), json!("unknown")] {
        let value = json!({"version":version, "nodes":[]});
        assert!(
            WorkflowDefinition::from_value_with_version(
                value.clone(),
                WorkflowDefinitionVersion::CURRENT
            )
            .is_err()
        );
        assert!(matches!(
            WorkflowDefinition::from_json_with_version(
                &value.to_string(),
                WorkflowDefinitionVersion::CURRENT
            ),
            Err(DefinitionParseError::JsonParse { .. })
        ));
    }
    let explicit = WorkflowDefinition::from_value_with_version(
        json!({"version":"2026-09-26", "nodes":[]}),
        WorkflowDefinitionVersion::CURRENT,
    )
    .unwrap();
    assert_eq!(explicit.version, WorkflowDefinitionVersion::V2026_09_26);
    assert!(matches!(
        WorkflowDefinition::from_json_with_version(
            r#"{"version":"2026-09-24","nodes":[]}"#,
            WorkflowDefinitionVersion::CURRENT
        ),
        Err(DefinitionParseError::LegacyVersion)
    ));
}

#[test]
fn child_json_parser_retains_duplicate_key_rejection() {
    for input in [
        r#"{"nodes":[],"nodes":[]}"#,
        r#"{"nodes":[],"version":null,"version":"2026-10-28"}"#,
        r#"{"nodes":[{"id":"n","kind":"opaque","config":{"value":1,"value":2}}]}"#,
    ] {
        let error =
            WorkflowDefinition::from_json_with_version(input, WorkflowDefinitionVersion::CURRENT)
                .unwrap_err();
        assert!(matches!(error, DefinitionParseError::JsonParse { .. }));
        assert!(error.to_string().contains("duplicate"), "{error}");
    }
    let root = r#"{"version":"2026-10-28","nodes":[],"nodes":[]}"#;
    assert!(
        WorkflowDefinition::from_json(root)
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
}

#[test]
fn inherited_versions_enforce_their_own_wire_features() {
    let value = json!({"nodes":[{"id":"n", "kind":"ordinary", "inputs":{
        "value":{"from":{"node":"source", "port":"value"}}
    }}]});
    let error = WorkflowDefinition::from_value_with_version(
        value.clone(),
        WorkflowDefinitionVersion::V2026_10_03,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("node-local inputs require version 2026-10-19")
    );
    assert!(
        WorkflowDefinition::from_value_with_version(value, WorkflowDefinitionVersion::CURRENT,)
            .is_ok()
    );
}
