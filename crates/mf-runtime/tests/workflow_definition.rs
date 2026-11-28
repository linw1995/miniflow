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
fn default_collapsed_clusters_require_a_supported_schema() {
    let value = json!({
        "version": "2026-11-11",
        "nodes": [{"id": "source", "kind": "fixture.source"}],
        "clusters": [{
            "label": "Work",
            "annotation": "Collapsed by default.",
            "nodes": ["source"],
            "collapsed": true
        }]
    });
    let parsed = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    assert_eq!(parsed.version, WorkflowDefinitionVersion::V2026_11_11);
    assert!(parsed.version.supports_cluster_collapsed());
    assert!(parsed.version.supports_child_streams());
    assert!(parsed.version.supports_nested_flows());
    assert!(parsed.version.supports_startup_inputs());
    assert!(parsed.version.supports_node_success_edges());
    assert!(parsed.version.supports_clusters());
    assert!(parsed.clusters[0].collapsed);
    let serialized = serde_json::to_value(&parsed).unwrap();
    assert_eq!(serialized["version"], "2026-11-11");
    assert_eq!(serialized["clusters"][0]["collapsed"], true);
    assert_eq!(
        serde_json::from_value::<WorkflowDefinition>(serialized).unwrap(),
        parsed
    );

    for version in [
        WorkflowDefinitionVersion::V2026_09_26,
        WorkflowDefinitionVersion::V2026_09_29,
        WorkflowDefinitionVersion::V2026_10_02,
        WorkflowDefinitionVersion::V2026_10_03,
        WorkflowDefinitionVersion::V2026_10_19,
        WorkflowDefinitionVersion::V2026_10_23,
        WorkflowDefinitionVersion::V2026_10_25,
        WorkflowDefinitionVersion::V2026_10_28,
    ] {
        assert!(!version.supports_cluster_collapsed());
        let mut old = value.clone();
        old["version"] = serde_json::to_value(version).unwrap();
        let error = WorkflowDefinition::from_json(&old.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("default-collapsed clusters require version 2026-11-11"),
            "{version:?}: {error}"
        );
        let mut programmatic = parsed.clone();
        programmatic.version = version;
        let error = serde_json::to_value(programmatic).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("default-collapsed clusters require version 2026-11-11"),
            "{version:?}: {error}"
        );
    }
}

#[test]
fn expanded_clusters_default_to_false_and_remain_old_schema_compatible() {
    for version in [
        "2026-10-23",
        "2026-10-25",
        "2026-10-28",
        "2026-11-11",
        "2026-11-13",
        "2026-11-28",
    ] {
        for collapsed in [None, Some(false)] {
            let mut value = json!({
                "version": version,
                "nodes": [{"id": "source", "kind": "fixture.source"}],
                "clusters": [{
                    "label": "Work", "annotation": "Expanded by default.", "nodes": ["source"]
                }]
            });
            if let Some(collapsed) = collapsed {
                value["clusters"][0]["collapsed"] = collapsed.into();
            }
            let parsed = WorkflowDefinition::from_json(&value.to_string()).unwrap();
            assert!(!parsed.clusters[0].collapsed);
            let serialized = serde_json::to_value(&parsed).unwrap();
            assert!(serialized["clusters"][0].get("collapsed").is_none());
            assert_eq!(
                serde_json::from_value::<WorkflowDefinition>(serialized).unwrap(),
                parsed
            );
        }
    }
}

fn context_input(binding: Value) -> Value {
    json!({"nodes":[
        {"id":"consumer", "kind":"test.sink", "inputs":{"given":binding}},
        {"id":"producer.part", "kind":"test.source"}
    ]})
}

#[test]
fn input_refs_are_canonical_for_every_supported_schema_generation() {
    let source = "producer.part.value.part";
    for version in [
        WorkflowDefinitionVersion::V2026_09_26,
        WorkflowDefinitionVersion::V2026_10_19,
        WorkflowDefinitionVersion::V2026_11_13,
        WorkflowDefinitionVersion::CURRENT,
    ] {
        let parsed = WorkflowDefinition::from_value_with_version(
            context_input(json!({"$ref":source,"path":""})),
            version,
        )
        .unwrap();
        assert_eq!(parsed.edges[0].from_node.as_str(), "producer.part");
        assert_eq!(parsed.edges[0].from_output, "value.part");
        let encoded = json!(parsed);
        assert_eq!(encoded["version"], json!(version));
        assert_eq!(
            encoded["nodes"][0]["inputs"]["given"],
            json!({"$ref":source})
        );
        assert_eq!(
            serde_json::from_value::<WorkflowDefinition>(encoded).unwrap(),
            parsed
        );
        let legacy = context_input(json!({"from":{"node":"producer.part","port":"value.part"}}));
        assert!(WorkflowDefinition::from_value_with_version(legacy, version).is_err());
        let endpoints = context_input(
            json!({"exclusive":[{"node":"producer.part","port":"a"},{"node":"producer.part","port":"b"}]}),
        );
        assert!(WorkflowDefinition::from_value_with_version(endpoints, version).is_err());
        let mut old_edges = json!({"nodes":[{"id":"producer.part", "kind":"test.source"}, {"id":"consumer", "kind":"test.sink"}]});
        old_edges["edges"] = json!([{"from_node":"producer.part","from_output":"value", "to_node":"consumer", "to_input":"given"}]);
        assert!(WorkflowDefinition::from_value_with_version(old_edges, version).is_err());
    }
}

#[test]
fn input_refs_reject_projections_templates_and_malformed_or_mixed_sources() {
    let source = json!({"$ref":"producer.part.value"});
    let legacy = json!({"from":{"node":"producer.part","port":"value"}});
    for invalid in [
        Value::Null,
        json!("producer.part.value"),
        json!({"$ref":1}),
        json!({"$ref":""}),
        json!({"$ref":"producer.part."}),
        json!({"$ref":"missing.value"}),
        json!({"$ref":"producer.part.value","path":"/item"}),
        json!({"$template":"{{ producer.part.value }}"}),
        legacy,
        json!({"$ref":"producer.part.value","exclusive":[]}),
        json!({"exclusive":[source, {"node":"producer.part","port":"other"}]}),
        json!({"exclusive":[source]}),
        json!({"exclusive":[source, source]}),
    ] {
        let error = WorkflowDefinition::from_value_with_version(
            context_input(invalid.clone()),
            WorkflowDefinitionVersion::CURRENT,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("consumer/inputs/given"),
            "{invalid}: {error}"
        );
    }
}

#[test]
fn input_refs_cannot_serialize_an_invalid_exclusive_group() {
    let mut graph = WorkflowDefinition::from_value_with_version(
        context_input(json!({"$ref":"producer.part.value"})),
        WorkflowDefinitionVersion::CURRENT,
    )
    .unwrap();
    graph.edges[0].exclusive = true;
    assert!(
        serde_json::to_value(&graph)
            .unwrap_err()
            .to_string()
            .contains("at least two sources")
    );
    graph.edges.push(graph.edges[0].clone());
    assert!(
        serde_json::to_value(graph)
            .unwrap_err()
            .to_string()
            .contains("duplicate candidate")
    );
}

#[test]
fn ambiguous_input_refs_are_never_guessed_on_read_or_write() {
    let mut input = context_input(json!({"$ref":"producer.part.value"}));
    input["version"] = json!(WorkflowDefinitionVersion::CURRENT);
    let mut graph: WorkflowDefinition = serde_json::from_value(input.clone()).unwrap();
    let overlapping = json!({"id":"producer", "kind":"other"});
    graph
        .nodes
        .push(serde_json::from_value(overlapping.clone()).unwrap());
    input["nodes"].as_array_mut().unwrap().push(overlapping);
    for error in [
        serde_json::to_value(graph).unwrap_err(),
        serde_json::from_value::<WorkflowDefinition>(input).unwrap_err(),
    ] {
        assert!(
            error.to_string().contains("ambiguous")
                && error.to_string().contains("producer.part.value"),
            "{error}"
        );
    }
}

#[test]
fn inherited_versions_enforce_their_own_wire_features() {
    let value = json!({"nodes":[{"id":"source", "kind":"source"},{"id":"n", "kind":"ordinary"}],
        "edges":[{"from_node":"source", "to_node":"n"}]});
    let error = WorkflowDefinition::from_value_with_version(
        value.clone(),
        WorkflowDefinitionVersion::V2026_10_03,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("node-success edges require version 2026-10-19")
    );
    assert!(
        WorkflowDefinition::from_value_with_version(value, WorkflowDefinitionVersion::V2026_11_13)
            .is_ok()
    );
}
