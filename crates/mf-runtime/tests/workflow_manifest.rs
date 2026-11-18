use mf_runtime::{
    CompiledGraph, MANIFEST_FRAMING_VERSION, MANIFEST_HEADER_BYTES, MANIFEST_MAGIC,
    MAX_MANIFEST_PADDING_BYTES, MAX_MANIFEST_PAYLOAD_BYTES, WorkflowInput, WorkflowInputSchema,
    WorkflowManifest, WorkflowManifestError,
};
use serde_json::{Value, json};
use std::error::Error;

fn graph(definition: Value, order: &[&str]) -> CompiledGraph {
    CompiledGraph {
        definition: serde_json::from_value(definition).unwrap(),
        execution_order: order.iter().map(|id| (*id).into()).collect(),
        relations: Default::default(),
        children: Default::default(),
    }
}

fn sample() -> WorkflowManifest {
    WorkflowManifest::from_graph(
        &(graph(
            json!({
                "version": "2026-10-03", "dependencies": {},
                "nodes": [{"id": "root.a/~", "kind": "fixture.input"}]
            }),
            &["root.a/~"],
        )),
        WorkflowInputSchema {
            inputs: [(
                "root.a/~".into(),
                [(
                    "port.a/~".into(),
                    WorkflowInput {
                        value_type: mf_runtime::ValueType::String,
                        required: false,
                        binding: Default::default(),
                    },
                )]
                .into(),
            )]
            .into(),
            stdin: [(
                "root.a/~".into(),
                mf_runtime::StdinRequirement::UnlessInput("port.a/~".into()),
            )]
            .into(),
        },
    )
    .unwrap()
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = MANIFEST_MAGIC.to_vec();
    bytes.extend_from_slice(&MANIFEST_FRAMING_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn inspection_persists_a_projection_without_compiler_relationships() {
    let manifest = sample();
    let payload = serde_json::to_value(&manifest).unwrap();
    assert_eq!(payload["version"], "2026-11-14");
    assert_eq!(
        payload["definition"],
        serde_json::to_value(&manifest.definition).unwrap()
    );
    assert_eq!(
        payload["sources"]["workflow.json"],
        serde_json::to_string(&manifest.definition).unwrap()
    );
    assert_eq!(
        payload["descriptor"]["description"],
        serde_json::to_value(manifest.description().unwrap()).unwrap()
    );
    for field in [
        "graph",
        "relations",
        "children",
        "typed_plan",
        "execution_domains",
    ] {
        assert!(payload.get(field).is_none());
        assert!(payload["descriptor"].get(field).is_none());
    }
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded, manifest);
    assert_eq!(
        decoded.node_definition("root.a/~"),
        manifest.node_definition("root.a/~")
    );
    for field in ["definition", "entry", "sources", "descriptor"] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err()
        );
    }
    let legacy = json!({"version":"2026-11-06", "graph":{}, "input_schema":{}});
    assert!(WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&legacy).unwrap())).is_err());
}

#[test]
fn node_inspection_has_no_execution_indices_and_validates_isolated_identity() {
    let mut manifest = sample();
    let original = manifest.workflow_id().unwrap();
    let mut description = manifest.description().unwrap();
    description.workflow_id = original.for_node_target("root.a/~").unwrap();
    manifest
        .descriptor
        .node_targets
        .push(mf_runtime::NodeTarget {
            id: "root.a/~".into(),
            kind: mf_runtime::NodeTargetKind::Task,
            worker_limit: None,
            inputs: manifest.descriptor.input_schema.clone(),
            outputs: Default::default(),
            context: Default::default(),
            original_workflow_id: original.clone(),
            description,
        });
    let payload = serde_json::to_value(&manifest).unwrap();
    assert!(
        payload["descriptor"]["node_targets"][0]
            .get("index")
            .is_none()
    );
    assert_eq!(
        WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap(),
        manifest
    );
    let mut unknown = payload;
    unknown["descriptor"]["node_targets"][0]["index"] = json!(0);
    assert!(WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&unknown).unwrap())).is_err());
    let mut duplicate = manifest.clone();
    duplicate
        .descriptor
        .node_targets
        .push(manifest.descriptor.node_targets[0].clone());
    assert!(matches!(
        duplicate.validate(),
        Err(WorkflowManifestError::Projection { .. })
    ));
    manifest.descriptor.node_targets[0].description.workflow_id = original;
    assert!(matches!(
        manifest.validate(),
        Err(WorkflowManifestError::Projection { .. })
    ));
}

#[test]
fn preserves_protocols_names_and_conditional_resources() {
    let manifest = sample();
    let mut bytes = manifest.to_bytes().unwrap();
    assert_eq!(&bytes[..8], b"MFMANIF\0");
    assert_eq!(&bytes[8..12], &1u32.to_le_bytes());
    assert_eq!(
        u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        (bytes.len() - MANIFEST_HEADER_BYTES) as u64
    );
    bytes.extend(vec![0; MAX_MANIFEST_PADDING_BYTES]);
    assert_eq!(WorkflowManifest::from_bytes(&bytes).unwrap(), manifest);
    bytes.push(0);
    assert!(matches!(
        WorkflowManifest::from_bytes(&bytes),
        Err(WorkflowManifestError::Framing { .. })
    ));
}

#[test]
fn rejects_corrupt_framing_and_lengths_without_allocating_declared_payloads() {
    let bytes = sample().to_bytes().unwrap();
    for end in [0, 7, 19, bytes.len() - 1] {
        assert!(matches!(
            WorkflowManifest::from_bytes(&bytes[..end]),
            Err(WorkflowManifestError::Framing { .. })
        ));
    }
    let mut invalid = bytes.clone();
    invalid[0] = 0;
    assert!(matches!(
        WorkflowManifest::from_bytes(&invalid),
        Err(WorkflowManifestError::Framing { .. })
    ));
    invalid = bytes.clone();
    invalid[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        WorkflowManifest::from_bytes(&invalid),
        Err(WorkflowManifestError::UnsupportedFraming { version: 2 })
    ));
    for length in [MAX_MANIFEST_PAYLOAD_BYTES as u64 + 1, u64::MAX] {
        invalid = bytes.clone();
        invalid[12..20].copy_from_slice(&length.to_le_bytes());
        assert!(matches!(
            WorkflowManifest::from_bytes(&invalid),
            Err(WorkflowManifestError::TooLarge { .. })
        ));
    }
    invalid = bytes;
    invalid.push(1);
    assert!(matches!(
        WorkflowManifest::from_bytes(&invalid),
        Err(WorkflowManifestError::Framing { .. })
    ));
}

#[test]
fn rejects_invalid_versions_and_duplicate_members_with_json_sources() {
    let payload = serde_json::to_string(&sample()).unwrap();
    for invalid in [
        payload.replace("2026-11-14", "2099-01-01"),
        payload.replacen("{", "{\"version\":\"2026-11-14\",", 1),
        payload.replace("\"required\":false", "\"required\":false,\"required\":true"),
        format!("{payload}{{}}"),
        "{".into(),
    ] {
        let error = WorkflowManifest::from_bytes(&frame(invalid.as_bytes())).unwrap_err();
        assert!(matches!(error, WorkflowManifestError::Json { .. }));
        assert!(error.source().unwrap().is::<serde_json::Error>());
    }
    let mut invalid = sample();
    invalid.definition.version = mf_runtime::WorkflowDefinitionVersion::V2026_09_29;
    invalid
        .definition
        .clusters
        .push(mf_runtime::ClusterDefinition {
            label: "Work".into(),
            annotation: "Requires a newer definition.".into(),
            nodes: vec!["root.a/~".into()],
            collapsed: false,
        });
    let error = invalid.validate().unwrap_err();
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<serde_json::Error>()
    );
}

#[test]
fn limits_the_combined_serialized_payload() {
    let mut manifest = sample();
    manifest.definition.nodes[0].config = json!({"opaque": "x".repeat(MAX_MANIFEST_PAYLOAD_BYTES)});
    assert!(matches!(
        manifest.to_bytes(),
        Err(WorkflowManifestError::TooLarge { .. })
    ));
}

#[test]
fn reports_exact_interface_drift_before_accepting_prepared_schemas() {
    let manifest = sample();
    let schema = manifest.descriptor.input_schema.clone();
    manifest.validate_prepared_schema(&schema).unwrap();
    for component in [
        "node",
        "port",
        "type",
        "required",
        "env",
        "default",
        "secret",
        "stdin",
        "condition",
    ] {
        let mut actual = schema.clone();
        match component {
            "node" => {
                actual.inputs.insert("extra".into(), Default::default());
            }
            "port" => actual.inputs.get_mut("root.a/~").unwrap().clear(),
            "stdin" => actual.stdin.clear(),
            "type" | "required" | "env" | "default" | "secret" => {
                let input = actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap();
                match component {
                    "type" => input.value_type = mf_runtime::ValueType::Int64,
                    "required" => input.required = true,
                    "env" => input.binding.env = Some("CONFIG_VALUE".into()),
                    "default" => input.binding.default = Some(json!("default")),
                    _ => input.binding.secret = true,
                }
            }
            _ => {
                actual
                    .stdin
                    .insert("root.a/~".into(), mf_runtime::StdinRequirement::Always);
            }
        }
        let error = manifest.validate_prepared_schema(&actual).unwrap_err();
        assert!(matches!(
            error,
            WorkflowManifestError::InterfaceDrift { .. }
        ));
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains(if component == "node" {
                "/extra"
            } else {
                "/root.a~1~0"
            }),
            "{diagnostic}"
        );
        if matches!(
            component,
            "port" | "type" | "required" | "env" | "default" | "secret"
        ) {
            assert!(diagnostic.contains("/port.a~1~0"), "{diagnostic}");
        }
    }
}
