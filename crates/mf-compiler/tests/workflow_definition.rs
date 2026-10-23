use mf_compiler::{
    DefinitionParseError, WorkflowDefinition, WorkflowDefinitionVersion, plan_definition,
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

    assert_eq!(definition.version, WorkflowDefinitionVersion::V2026_09_26);
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
fn loop_body_clusters_are_preserved_in_descriptions() {
    let definition = WorkflowDefinition::from_json(
        r#"{
            "version": "2026-10-23",
            "dependencies": {},
            "nodes": [{
                "id": "repeat",
                "kind": "workflow.loop",
                "loop": {
                    "max_iterations": 1,
                    "variables": [{"name": "count", "type": "int"}],
                    "body": {
                        "nodes": [{"id": "step", "kind": "fixture.task"}],
                        "clusters": [{
                            "label": "Body step",
                            "annotation": "The only body transformation.",
                            "nodes": ["step"]
                        }]
                    }
                }
            }]
        }"#,
    )
    .unwrap();
    let plan = plan_definition(&definition).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(description.clusters.len(), 0);
    assert_eq!(description.loop_bodies.len(), 1);
    assert_eq!(description.loop_bodies[0].clusters.len(), 1);
    assert_eq!(description.loop_bodies[0].clusters[0].label, "Body step");
    assert_eq!(
        description.loop_bodies[0].clusters[0].nodes,
        vec!["step".to_owned()]
    );
    let mut legacy = definition;
    legacy.version = WorkflowDefinitionVersion::V2026_10_19;
    let error = plan_definition(&legacy).unwrap_err().to_string();
    assert!(
        error.contains("node clusters require version 2026-10-23"),
        "{error}"
    );
    assert!(
        serde_json::to_string(&legacy)
            .unwrap_err()
            .to_string()
            .contains("node clusters require version 2026-10-23")
    );
}

#[test]
fn iteration_body_clusters_are_preserved_in_descriptions() {
    let definition = WorkflowDefinition::from_json(
        r#"{
            "version": "2026-10-23",
            "dependencies": {},
            "nodes": [{
                "id": "iterate",
                "kind": "builtin.iteration",
                "config": {
                    "body": {
                        "nodes": [{"id": "step", "kind": "fixture.task"}],
                        "result": {"node": "step", "port": "value"},
                        "clusters": [{
                            "label": "Item step",
                            "annotation": "The body executed for every item.",
                            "nodes": ["step"]
                        }]
                    }
                }
            }]
        }"#,
    )
    .unwrap();
    let plan = plan_definition(&definition).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(description.loop_bodies.len(), 1);
    assert_eq!(description.loop_bodies[0].clusters.len(), 1);
    assert_eq!(description.loop_bodies[0].clusters[0].label, "Item step");
    assert_eq!(
        description.loop_bodies[0].clusters[0].nodes,
        vec!["step".to_owned()]
    );
}
#[test]
fn nested_iteration_clusters_inherit_the_root_definition_version() {
    use serde_json::json;
    let mut node = json!({
        "id":"iterate", "kind":"builtin.iteration", "config":{"body":{
            "nodes":[{"id":"step","kind":"fixture.task"}],
            "result":{"node":"step","port":"value"},
            "clusters":[{"label":"Nested item","annotation":"Transform every nested item.","nodes":["step"]}]
        }}
    });
    for id in ["inner", "outer"] {
        node = json!({
            "id":id,"kind":"workflow.loop","loop":{
                "max_iterations":1,"variables":[{"name":"count","type":"int"}],
                "body":{"nodes":[node]}
            }
        });
    }
    let definition = serde_json::from_value(json!({
        "version":"2026-10-23","dependencies":{},"nodes":[node]
    }))
    .unwrap();
    let plan = plan_definition(&definition).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    let body = description
        .body(&["outer".into(), "inner".into(), "iterate".into()])
        .unwrap();
    assert_eq!(body.clusters.len(), 1);
    assert_eq!(body.clusters[0].label, "Nested item");
    assert_eq!(body.clusters[0].nodes, ["step"]);
    assert!(
        description
            .loop_bodies
            .iter()
            .filter(|body| body.path.len() < 3)
            .all(|body| body.clusters.is_empty())
    );
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
    assert_eq!(definition.dependencies.len(), 1);
    assert_eq!(
        definition,
        WorkflowDefinition::from_json(&serde_json::to_string(&definition).unwrap()).unwrap()
    );
}

#[test]
fn clusters_require_the_supported_definition_version_and_round_trip() {
    let mut value = serde_json::json!({
        "version": "2026-10-23",
        "dependencies": {},
        "nodes": [
            {"id": "read", "kind": "fixture.read"},
            {"id": "parse", "kind": "fixture.parse"}
        ],
        "clusters": [{
            "label": "Load and validate",
            "annotation": "Read the input and enforce its schema.",
            "nodes": ["read", "parse"]
        }]
    });
    let definition = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    assert_eq!(definition.version, WorkflowDefinitionVersion::V2026_10_23);
    assert_eq!(definition.clusters.len(), 1);
    assert_eq!(
        definition,
        WorkflowDefinition::from_json(&serde_json::to_string(&definition).unwrap()).unwrap()
    );
    let plan = plan_definition(&definition).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(
        description.version,
        mf_telemetry::description::WorkflowDescriptionVersion::V2026_10_23
    );
    assert_eq!(description.clusters.len(), 1);
    assert_eq!(description.clusters[0].label, "Load and validate");

    for version in ["2026-10-19", "2026-10-03"] {
        value["version"] = serde_json::json!(version);
        let error = WorkflowDefinition::from_json(&value.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("node clusters require version 2026-10-23"),
            "{version}: {error}"
        );
    }
    value["version"] = serde_json::json!("2026-10-23");
}

#[test]
fn cluster_validation_rejects_unknown_duplicate_and_synthetic_members() {
    let mut value = serde_json::json!({
        "version": "2026-10-23",
        "dependencies": {},
        "nodes": [
            {"id": "read", "kind": "fixture.read"},
            {"id": "parse", "kind": "fixture.parse"}
        ],
        "clusters": [{
            "label": "Load",
            "annotation": "Read the input.",
            "nodes": ["read"]
        }]
    });
    let base = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    assert!(plan_definition(&base).is_ok());

    let mut unknown = base.clone();
    unknown.clusters[0].nodes = vec!["missing".into()];
    let error = plan_definition(&unknown).unwrap_err().to_string();
    assert!(error.contains("unknown node"), "{error}");

    let mut duplicate = base.clone();
    duplicate.clusters.push(duplicate.clusters[0].clone());
    let error = plan_definition(&duplicate).unwrap_err().to_string();
    assert!(error.contains("more than one cluster"), "{error}");

    let mut synthetic = base;
    synthetic.clusters[0].nodes = vec!["%loop".into()];
    let error = plan_definition(&synthetic).unwrap_err().to_string();
    assert!(error.contains("synthetic nodes"), "{error}");

    value["clusters"][0]["label"] = serde_json::json!("");
    let empty_label = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let error = plan_definition(&empty_label).unwrap_err().to_string();
    assert!(error.contains("cluster label"), "{error}");
}

#[test]
fn programmatic_clusters_enforce_versions_and_metadata_limits() {
    use mf_telemetry::description::{
        MAX_CLUSTER_ANNOTATION_BYTES, MAX_CLUSTER_LABEL_BYTES, MAX_CLUSTERS_PER_SCOPE,
    };
    use serde_json::json;
    let mut base: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-23","dependencies":{},
        "nodes":[{"id":"read","kind":"fixture.read"}],
        "clusters":[{"label":"Load","annotation":"Read the input.","nodes":["read"]}]
    }))
    .unwrap();
    base.clusters[0].label = "é".repeat(MAX_CLUSTER_LABEL_BYTES / 2);
    base.clusters[0].annotation = format!("\n\t{}", "a".repeat(MAX_CLUSTER_ANNOTATION_BYTES - 2));
    assert!(plan_definition(&base).is_ok());
    assert_eq!(
        base,
        WorkflowDefinition::from_json(&serde_json::to_string(&base).unwrap()).unwrap()
    );

    for version in [
        WorkflowDefinitionVersion::V2026_10_03,
        WorkflowDefinitionVersion::V2026_10_19,
    ] {
        let mut invalid = base.clone();
        invalid.version = version;
        let error = plan_definition(&invalid).unwrap_err().to_string();
        assert!(
            error.contains("node clusters require version 2026-10-23"),
            "{error}"
        );
        let error = serde_json::to_string(&invalid).unwrap_err().to_string();
        assert!(
            error.contains("node clusters require version 2026-10-23"),
            "{error}"
        );
    }
    for label in [
        format!("{}!", base.clusters[0].label),
        "two\nlines".into(),
        "escape\x1b".into(),
    ] {
        let mut invalid = base.clone();
        invalid.clusters[0].label = label;
        let error = plan_definition(&invalid).unwrap_err().to_string();
        assert!(error.contains("cluster label"), "{error}");
    }
    for annotation in [
        " \n\t".into(),
        "escape\x1b".into(),
        "a".repeat(MAX_CLUSTER_ANNOTATION_BYTES + 1),
    ] {
        let mut invalid = base.clone();
        invalid.clusters[0].annotation = annotation;
        let error = plan_definition(&invalid).unwrap_err().to_string();
        assert!(error.contains("cluster annotation"), "{error}");
    }
    let mut empty = base.clone();
    empty.clusters[0].nodes.clear();
    assert!(
        plan_definition(&empty)
            .unwrap_err()
            .to_string()
            .contains("at least one node")
    );

    let mut oversized = base.clone();
    oversized.nodes = (0..=MAX_CLUSTERS_PER_SCOPE)
        .map(|index| {
            let mut node = base.nodes[0].clone();
            node.id = format!("node-{index}").into();
            node
        })
        .collect();
    oversized.clusters = oversized
        .nodes
        .iter()
        .map(|node| {
            let mut cluster = base.clusters[0].clone();
            cluster.nodes = vec![node.id.clone()];
            cluster
        })
        .collect();
    assert!(
        plan_definition(&oversized)
            .unwrap_err()
            .to_string()
            .contains("too many node clusters")
    );
}
