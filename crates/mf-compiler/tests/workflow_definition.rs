use mf_compiler::definition::{
    DefinitionParseError, WorkflowDefinition, WorkflowDefinitionVersion,
};

#[test]
fn parses_a_versioned_workflow_definition() {
    let definition = WorkflowDefinition::from_json(
        r#"{
            "version": "2026-09-26", "dependencies": {},
            "nodes": [{ "id": "source", "kind": "constant" }]
        }"#,
    )
    .unwrap();

    assert_eq!(definition.version, WorkflowDefinitionVersion::CURRENT);
    assert_eq!(definition.nodes[0].id.as_str(), "source");
    assert_eq!(definition.nodes[0].config, serde_json::json!({}));
    assert_eq!(
        serde_json::to_value(&definition).unwrap()["nodes"][0]["id"],
        "source"
    );
    assert!(definition.edges.is_empty());
    assert!(definition.outputs.is_empty());
}

#[test]
fn rejects_unsupported_definition_versions() {
    let error =
        WorkflowDefinition::from_json(r#"{"version":"2027-01-01","nodes":[]}"#).unwrap_err();

    assert!(matches!(&error, DefinitionParseError::JsonParse { .. }));
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .to_string()
            .contains("unknown variant `2027-01-01`")
    );
}

#[test]
fn rejects_invalid_version_dates() {
    let error =
        WorkflowDefinition::from_json(r#"{"version":"2026-02-30","nodes":[]}"#).unwrap_err();

    assert!(matches!(&error, DefinitionParseError::JsonParse { .. }));
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .to_string()
            .contains("unknown variant `2026-02-30`")
    );
}

#[test]
fn rejects_non_date_version_formats() {
    let error = WorkflowDefinition::from_json(r#"{"version":1,"nodes":[]}"#).unwrap_err();

    assert!(matches!(error, DefinitionParseError::JsonParse { .. }));
}

#[test]
fn reports_malformed_definition_fields() {
    let error = WorkflowDefinition::from_json(
        r#"{"version": "2026-09-26", "dependencies": {},"nodez":[]}"#,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("invalid workflow definition JSON:")
    );
    assert!(
        std::error::Error::source(&error)
            .unwrap()
            .to_string()
            .contains("unknown field `nodez`")
    );
}

#[test]
fn dependencies_validate_sources_and_preserve_features() {
    use serde_json::json;
    let mut value = json!({"version":"2026-09-26", "dependencies": {
        "remote": {"package":"sample-nodes", "version":"1.2", "features":["json"], "default-features":false},
        "local": {"package":"local-nodes", "path":"./nodes"},
        "git": {"package":"git-nodes", "git":"https://example.org/nodes", "rev":"0123456789abcdef0123456789abcdef01234567"}
    }, "nodes":[]});
    let definition = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    assert!(!definition.dependencies["remote"].default_features);
    assert!(definition.dependencies["local"].default_features);
    assert_eq!(
        definition,
        WorkflowDefinition::from_json(&serde_json::to_string(&definition).unwrap()).unwrap()
    );
    for invalid in [
        json!({"package":"n"}),
        json!({"package":"n","path":".","version":"1"}),
        json!({"package":"n","git":"url","rev":"main"}),
        json!({"package":"n","path":".","surprise":true}),
    ] {
        value["dependencies"]["remote"] = invalid;
        assert!(
            WorkflowDefinition::from_json(&value.to_string())
                .unwrap_err()
                .to_string()
                .contains("dependencies.remote")
        );
    }
    value.as_object_mut().unwrap().remove("dependencies");
    assert!(WorkflowDefinition::from_json(&value.to_string()).is_err());
}

#[test]
fn old_schema_explains_migration() {
    let error =
        WorkflowDefinition::from_json(r#"{"version":"2026-09-24","nodes":[]}"#).unwrap_err();
    assert!(error.to_string().contains("2026-09-26"));
    assert!(error.to_string().contains("dependencies"));
}

#[test]
fn documented_example_round_trips_with_dependencies() {
    let definition =
        WorkflowDefinition::from_json(include_str!("../../../examples/hello-workflow.json"))
            .unwrap();
    assert_eq!(definition.dependencies.len(), 2);
    assert_eq!(
        definition,
        WorkflowDefinition::from_json(&serde_json::to_string(&definition).unwrap()).unwrap()
    );
}
