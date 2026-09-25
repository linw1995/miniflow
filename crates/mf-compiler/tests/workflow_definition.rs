use mf_compiler::definition::{
    DefinitionParseError, WorkflowDefinition, WorkflowDefinitionVersion,
};

#[test]
fn parses_a_versioned_workflow_definition() {
    let definition = WorkflowDefinition::from_json(
        r#"{
            "version": "2026-09-24",
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
    let error =
        WorkflowDefinition::from_json(r#"{"version":"2026-09-24","nodez":[]}"#).unwrap_err();

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
