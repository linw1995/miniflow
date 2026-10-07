use mf_runtime::{
    MANIFEST_FRAMING_VERSION, MANIFEST_HEADER_BYTES, MANIFEST_MAGIC, MAX_MANIFEST_PADDING_BYTES,
    MAX_MANIFEST_PAYLOAD_BYTES, WorkflowInput, WorkflowInputSchema, WorkflowInterface,
    WorkflowInterfaceVersion, WorkflowManifest, WorkflowManifestError, WorkflowManifestVersion,
};
use mf_telemetry::{
    description::{
        ExecutionDescription, ExecutionMode, NodeDescription, WorkflowDescription,
        WorkflowDescriptionVersion,
    },
    identity::WorkflowId,
};
use std::error::Error;

fn sample() -> WorkflowManifest {
    let workflow_id = WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap();
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_10_07,
        description: WorkflowDescription {
            version: WorkflowDescriptionVersion::V2026_10_03,
            workflow_id: workflow_id.clone(),
            execution: Some(ExecutionDescription {
                mode: ExecutionMode::Single,
                event_schema_version: mf_telemetry::LOOP_EVENT_SCHEMA_VERSION,
            }),
            nodes: vec![NodeDescription {
                id: "root.a/~".into(),
                kind: "fixture.input".into(),
            }],
            data_edges: vec![],
            control_edges: vec![],
            execution_order: vec!["root.a/~".into()],
            loop_bodies: vec![],
        },
        interface: WorkflowInterface {
            version: WorkflowInterfaceVersion::V2026_10_03,
            workflow_id,
            schema: WorkflowInputSchema {
                inputs: [(
                    "root.a/~".into(),
                    [(
                        "port.a/~".into(),
                        WorkflowInput {
                            value_type: mf_runtime::ValueType::String,
                            required: false,
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
        },
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = MANIFEST_MAGIC.to_vec();
    bytes.extend_from_slice(&MANIFEST_FRAMING_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
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
        payload.replace("2026-10-07", "2099-01-01"),
        payload.replacen("{", "{\"version\":\"2026-10-07\",", 1),
        payload.replace("\"required\":false", "\"required\":false,\"required\":true"),
        format!("{payload}{{}}"),
        "{".into(),
    ] {
        let error = WorkflowManifest::from_bytes(&frame(invalid.as_bytes())).unwrap_err();
        assert!(matches!(error, WorkflowManifestError::Json { .. }));
        assert!(error.source().unwrap().is::<serde_json::Error>());
    }
}

#[test]
fn rejects_inconsistent_graphs_and_interfaces_on_read_and_write() {
    let mut manifest = sample();
    manifest.interface.workflow_id =
        WorkflowId::try_from(format!("sha256:{}", "b".repeat(64))).unwrap();
    assert!(matches!(
        manifest.to_bytes(),
        Err(WorkflowManifestError::Interface { .. })
    ));
    let bytes = frame(&serde_json::to_vec(&manifest).unwrap());
    assert!(matches!(
        WorkflowManifest::from_bytes(&bytes),
        Err(WorkflowManifestError::Interface { .. })
    ));
    manifest = sample();
    manifest.description.execution_order.clear();
    let error = manifest.to_bytes().unwrap_err();
    assert!(matches!(error, WorkflowManifestError::Graph { .. }));
    assert!(error.source().unwrap().is::<mf_telemetry::ContractError>());
    manifest = sample();
    manifest.interface.schema.inputs.clear();
    assert!(matches!(
        manifest.to_bytes(),
        Err(WorkflowManifestError::Interface { .. })
    ));
}

#[test]
fn limits_the_combined_serialized_payload() {
    let mut manifest = sample();
    manifest.description.nodes[0].kind = "x".repeat(MAX_MANIFEST_PAYLOAD_BYTES);
    assert!(matches!(
        manifest.to_bytes(),
        Err(WorkflowManifestError::TooLarge { .. })
    ));
}

#[test]
fn reports_exact_interface_drift_before_accepting_prepared_schemas() {
    let manifest = sample();
    let schema = manifest.interface.schema.clone();
    manifest.validate_prepared_schema(&schema).unwrap();
    for component in ["node", "port", "type", "required", "stdin", "condition"] {
        let mut actual = schema.clone();
        match component {
            "node" => {
                actual.inputs.insert("extra".into(), Default::default());
            }
            "port" => {
                actual.inputs.get_mut("root.a/~").unwrap().clear();
            }
            "type" => {
                actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap()
                    .value_type = mf_runtime::ValueType::Int64;
            }
            "required" => {
                actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap()
                    .required = true;
            }
            "stdin" => {
                actual.stdin.clear();
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
        if matches!(component, "port" | "type" | "required") {
            assert!(diagnostic.contains("/port.a~1~0"), "{diagnostic}");
        }
    }
}
