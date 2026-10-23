use mf_runtime::{
    MANIFEST_FRAMING_VERSION, MANIFEST_HEADER_BYTES, MANIFEST_MAGIC, MAX_MANIFEST_PADDING_BYTES,
    MAX_MANIFEST_PAYLOAD_BYTES, WorkflowInput, WorkflowInputSchema, WorkflowInterfaceVersion,
    WorkflowManifest, WorkflowManifestError, WorkflowManifestVersion,
};
use mf_telemetry::identity::WorkflowId;
use std::error::Error;

#[test]
fn compiled_manifests_derive_views_from_original_facts() {
    let config = serde_json::json!({
        "model": {"$ref": "parameters.model"}, "temperature": 0.25,
        "options": {"labels": ["first", "second"]}
    });
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-10-03", "dependencies": {},
        "nodes": [{"id": "root.a/~", "kind": "fixture.input", "config": config}]
    }))
    .unwrap();
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_10_16,
        definition,
        execution_order: vec!["root.a/~".into()],
        loop_orders: vec![],
        input_schema: sample().input_schema,
    };
    let payload = serde_json::to_value(&manifest).unwrap();
    assert_eq!(payload["version"], "2026-10-16");
    for derived in ["description", "interface", "workflow_id"] {
        assert!(payload.get(derived).is_none());
    }
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded, manifest);
    let node = decoded.node_definition("root.a/~").unwrap();
    assert_eq!(node.kind, "fixture.input");
    assert_eq!(node.config, config);
    assert!(std::ptr::eq(node, &decoded.definition.nodes[0]));
    assert!(decoded.node_definition("root.a").is_none());
    let identity = WorkflowId::from_definition(&decoded.definition, &["root.a/~".into()]).unwrap();
    let mut expected_graph = sample().description().unwrap();
    expected_graph.workflow_id = identity.clone();
    let mut expected_interface = sample().interface().unwrap();
    expected_interface.workflow_id = identity.clone();
    assert_eq!(decoded.description().unwrap(), expected_graph);
    assert_eq!(decoded.interface().unwrap(), expected_interface);
    assert_eq!(decoded.workflow_id().unwrap(), identity);
    let description = decoded.description().unwrap();
    assert_eq!(
        description.event_schema_version(),
        mf_telemetry::LOOP_EVENT_SCHEMA_VERSION
    );
    assert!(
        serde_json::to_value(&description).unwrap()["execution"]
            .get("event_schema_version")
            .is_none()
    );
    let mut stream = decoded.clone();
    let definition = &mut stream.definition;
    definition.execution =
        Some(serde_json::from_value(serde_json::json!({"mode": "stream"})).unwrap());
    assert_eq!(
        stream.description().unwrap().event_schema_version(),
        mf_telemetry::STREAM_EVENT_SCHEMA_VERSION
    );
    let input_schema = &mut stream.input_schema;
    input_schema
        .inputs
        .get_mut("root.a/~")
        .unwrap()
        .get_mut("port.a/~")
        .unwrap()
        .binding
        .default = Some(serde_json::json!("fallback"));
    assert_eq!(
        stream.interface().unwrap().version,
        WorkflowInterfaceVersion::V2026_10_16
    );

    for missing in ["definition", "execution_order", "input_schema"] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(missing);
        assert!(matches!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())),
            Err(WorkflowManifestError::Json { .. })
        ));
    }
    for version in ["2026-10-07", "2099-01-01"] {
        let mut invalid = payload.clone();
        invalid["version"] = version.into();
        assert!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err()
        );
    }
    let mut invalid = payload;
    invalid["description"] = serde_json::to_value(expected_graph).unwrap();
    assert!(WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err());
}

#[test]
fn nested_loop_views_use_frozen_orders_and_reject_inconsistent_facts() {
    let definition = serde_json::from_value(serde_json::json!({
        "version": "2026-09-29", "dependencies": {},
        "nodes": [{"id": "repeat", "kind": "workflow.loop", "loop": {
            "max_iterations": 1, "variables": [], "body": {
                "nodes": [{"id": "inner", "kind": "workflow.loop", "loop": {
                    "max_iterations": 1, "variables": [], "body": {"nodes": [], "edges": []}
                }}], "edges": []
            }
        }}]
    }))
    .unwrap();
    let manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_10_16,
        definition,
        execution_order: vec!["repeat".into()],
        loop_orders: vec![
            mf_runtime::LoopExecutionOrder {
                path: vec!["repeat".into()],
                execution_order: vec!["%loop".into(), "inner".into()],
            },
            mf_runtime::LoopExecutionOrder {
                path: vec!["repeat".into(), "inner".into()],
                execution_order: vec!["%loop".into()],
            },
        ],
        input_schema: WorkflowInputSchema::default(),
    };
    let decoded = WorkflowManifest::from_bytes(&manifest.to_bytes().unwrap()).unwrap();
    let graph = decoded.description().unwrap();
    assert_eq!(graph.loop_bodies.len(), 2);
    assert_eq!(
        graph.loop_bodies[0]
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["%loop", "inner"]
    );
    assert_eq!(graph.loop_bodies[1].path, ["repeat", "inner"]);
    let mut reordered = manifest.clone();
    let loop_orders = &mut reordered.loop_orders;
    loop_orders[0].execution_order.reverse();
    assert_eq!(
        reordered.description().unwrap().loop_bodies[0]
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        ["inner", "%loop"]
    );
    for case in [
        "missing",
        "duplicate",
        "unknown",
        "wrong_node",
        "wrong_root",
        "duplicate_node",
        "duplicate_order",
        "incomplete",
        "schema",
    ] {
        let mut invalid = manifest.clone();
        let WorkflowManifest {
            definition,
            execution_order,
            loop_orders,
            input_schema,
            ..
        } = &mut invalid;
        match case {
            "missing" => {
                loop_orders.remove(0);
            }
            "duplicate" => loop_orders.push(loop_orders[0].clone()),
            "unknown" => loop_orders.push(mf_runtime::LoopExecutionOrder {
                path: vec!["unknown".into()],
                execution_order: vec!["%loop".into()],
            }),
            "wrong_node" => loop_orders[1].execution_order[0] = "unknown".into(),
            "wrong_root" => execution_order[0] = "unknown".into(),
            "duplicate_node" => {
                let mut duplicate = definition.nodes[0].clone();
                duplicate.loop_definition = None;
                definition.nodes.push(duplicate);
            }
            "duplicate_order" => {
                definition.nodes.push(
                    serde_json::from_value(
                        serde_json::json!({"id": "other", "kind": "fixture.node"}),
                    )
                    .unwrap(),
                );
                execution_order.push("repeat".into());
            }
            "incomplete" => execution_order.clear(),
            "schema" => *input_schema = sample().input_schema,
            _ => unreachable!(),
        }
        assert!(invalid.to_bytes().is_err(), "{case}");
        assert!(
            WorkflowManifest::from_bytes(&frame(&serde_json::to_vec(&invalid).unwrap())).is_err(),
            "{case}"
        );
    }
}

fn sample() -> WorkflowManifest {
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_10_16,
        definition: serde_json::from_value(serde_json::json!({
            "version": "2026-10-03", "dependencies": {},
            "nodes": [{"id": "root.a/~", "kind": "fixture.input"}]
        }))
        .unwrap(),
        execution_order: vec!["root.a/~".into()],
        loop_orders: vec![],
        input_schema: WorkflowInputSchema {
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
        payload.replace("2026-10-16", "2099-01-01"),
        payload.replacen("{", "{\"version\":\"2026-10-16\",", 1),
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
fn limits_the_combined_serialized_payload() {
    let mut manifest = sample();
    manifest.definition.nodes[0].kind = "x".repeat(MAX_MANIFEST_PAYLOAD_BYTES);
    assert!(matches!(
        manifest.to_bytes(),
        Err(WorkflowManifestError::TooLarge { .. })
    ));
}

#[test]
fn reports_exact_interface_drift_before_accepting_prepared_schemas() {
    let manifest = sample();
    let schema = manifest.input_schema.clone();
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
            "env" | "default" | "secret" => {
                let binding = &mut actual
                    .inputs
                    .get_mut("root.a/~")
                    .unwrap()
                    .get_mut("port.a/~")
                    .unwrap()
                    .binding;
                match component {
                    "env" => binding.env = Some("CONFIG_VALUE".into()),
                    "default" => binding.default = Some(serde_json::json!("default")),
                    _ => binding.secret = true,
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
