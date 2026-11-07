use mf_telemetry::{
    Count,
    description::WorkflowDescription,
    event::{Event, LifecycleEvent, LoopPathEntry},
    identity::{RunId, WorkflowId},
    transport::EventSequence,
    wire::{TraceContext, WireRecord},
};
use serde_json::{Value, json};

fn record(name: &str) -> WireRecord {
    serde_json::from_str(match name {
        "start" => include_str!("fixtures/workflow-started.json"),
        "node-start" => include_str!("fixtures/node-started.json"),
        "success" => include_str!("fixtures/node-succeeded.json"),
        "finish" => include_str!("fixtures/workflow-succeeded.json"),
        "failure" => include_str!("fixtures/preparation-failed.json"),
        "failed-finish" => include_str!("fixtures/workflow-failed.json"),
        "skip" => include_str!("fixtures/node-skipped.json"),
        _ => panic!("unknown fixture"),
    })
    .unwrap()
}

fn graph() -> WorkflowDescription {
    WorkflowDescription::from_json(include_bytes!("fixtures/description.json")).unwrap()
}

fn branch_graph() -> WorkflowDescription {
    let mut value = serde_json::to_value(graph()).unwrap();
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id":"branch/target", "kind":"fixture.echo"
    }));
    value["execution_order"] = json!(["load.order", "branch/target"]);
    value["control_edges"] = json!([{"from_node":"load.order", "from_output":"branch.false", "to_node":"branch/target"}]);
    WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn canonical_identity_matches_independent_golden_sha256() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/identity-input.json")).unwrap();
    let order: Vec<String> = serde_json::from_value(fixture["execution_order"].clone()).unwrap();
    let id = WorkflowId::from_definition(&fixture["definition"], &order).unwrap();
    assert_eq!(
        id.as_str(),
        include_str!("fixtures/identity-digest.txt").trim()
    );
    assert_eq!(id, graph().workflow_id);
    let mut changed = fixture["definition"].clone();
    changed["nodes"][0]["config"]["a"]["number"] = json!(9007199254740992_i64);
    assert_ne!(id, WorkflowId::from_definition(&changed, &order).unwrap());
}

#[test]
fn object_order_is_irrelevant_but_array_order_and_numeric_representation_are_preserved() {
    let first: Value = serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[1,2]}"#).unwrap();
    let second: Value = serde_json::from_str(r#"{"a":[1,2],"z":{"a":1,"b":2}}"#).unwrap();
    let order = vec!["a".to_owned(), "b".to_owned()];
    let id = WorkflowId::from_definition(&first, &order).unwrap();
    assert_eq!(id, WorkflowId::from_definition(&second, &order).unwrap());
    assert_ne!(
        id,
        WorkflowId::from_definition(&first, &["b".into(), "a".into()]).unwrap()
    );
    let mut changed = first.clone();
    changed["a"] = json!([2, 1]);
    assert_ne!(id, WorkflowId::from_definition(&changed, &order).unwrap());
    changed = first;
    changed["z"]["a"] = json!(1.0);
    assert_ne!(id, WorkflowId::from_definition(&changed, &order).unwrap());
}

#[test]
fn run_ids_are_fresh_canonical_v4_uuids() {
    let first = RunId::new();
    assert_ne!(first, RunId::new());
    assert_eq!(first, RunId::try_from(first.to_string()).unwrap());
    for invalid in [
        "",
        "00000000-0000-0000-0000-000000000000",
        "12345678-1234-1234-9234-123456789abc",
        "12345678123442349234123456789abc",
        "12345678-1234-4234-9234-123456789ABC",
    ] {
        assert!(RunId::try_from(invalid.to_owned()).is_err(), "{invalid}");
    }
    for invalid in ["sha256:a", "sha256:GG", "", "sha512:abcdef"] {
        assert!(WorkflowId::try_from(invalid.to_owned()).is_err());
    }
}

#[test]
fn all_event_fixtures_round_trip_with_exact_attribute_and_body_mapping() {
    for name in [
        "start",
        "node-start",
        "success",
        "finish",
        "failure",
        "failed-finish",
        "skip",
    ] {
        let wire = record(name);
        let event = wire.decode().unwrap();
        assert_eq!(
            wire,
            WireRecord::from_event(&event, wire.time_unix_nano, None).unwrap(),
            "{name}"
        );
        if name != "skip" {
            event.validate_for(&graph()).unwrap();
        }
        assert!(!wire.attributes.contains_key("mf.node.attempt"));
        assert!(!wire.body.as_object().unwrap().contains_key("node"));
    }
}

#[test]
fn loop_pass_wire_records_use_the_current_schema_and_reject_retired_formats() {
    let event = LifecycleEvent {
        workflow_id: graph().workflow_id,
        run_id: RunId::new(),
        sequence: Count::try_from(2).unwrap(),
        event: Event::LoopPassStarted {
            path: vec![LoopPathEntry::Loop {
                loop_id: "repeat".into(),
                index: Count::ZERO,
            }],
            elapsed_ns: Count::try_from(1).unwrap(),
        },
    };
    let wire = WireRecord::from_event(&event, 1, None).unwrap();
    assert_eq!(
        wire.schema_version().unwrap(),
        mf_telemetry::EVENT_SCHEMA_VERSION
    );
    for version in 1..=6 {
        let mut retired = wire.clone();
        retired.attributes["mf.schema.version"] = json!(version);
        assert!(retired.decode().is_err());
    }
    assert_eq!(wire.decode().unwrap(), event);
}

#[test]
fn descriptions_validate_cluster_metadata_and_version() {
    let mut value = json!({
        "version": "2026-10-23",
        "workflow_id": graph().workflow_id,
        "execution": {"mode": "single"},
        "nodes": [
            {"id": "read", "kind": "fixture.read"},
            {"id": "parse", "kind": "fixture.parse"}
        ],
        "data_edges": [],
        "control_edges": [],
        "execution_order": ["read", "parse"],
        "clusters": [{
            "label": "Load and validate",
            "annotation": "Read the input and enforce its schema.",
            "nodes": ["read", "parse"]
        }]
    });
    let description = WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(description.clusters.len(), 1);
    assert_eq!(description.clusters[0].nodes, ["read", "parse"]);

    for version in ["2026-10-19", "2026-10-03"] {
        let mut invalid = value.clone();
        invalid["version"] = json!(version);
        assert!(
            WorkflowDescription::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{version}"
        );
    }

    let mut unknown = value.clone();
    unknown["clusters"][0]["nodes"] = json!(["missing"]);
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let mut duplicate = value.clone();
    duplicate["clusters"]
        .as_array_mut()
        .unwrap()
        .push(value["clusters"][0].clone());
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&duplicate).unwrap()).is_err());

    let mut synthetic = value.clone();
    synthetic["clusters"][0]["nodes"] = json!(["%loop"]);
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&synthetic).unwrap()).is_err());

    let mut empty = value.clone();
    empty["clusters"][0]["nodes"] = json!([]);
    let error = WorkflowDescription::from_json(&serde_json::to_vec(&empty).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("node cluster must not be empty"), "{error}");

    let count = mf_telemetry::description::MAX_CLUSTERS_PER_SCOPE + 1;
    let mut oversized = value.clone();
    oversized["nodes"] = (0..count)
        .map(|index| json!({"id":format!("node-{index}"),"kind":"fixture.read"}))
        .collect();
    oversized["execution_order"] = (0..count)
        .map(|index| json!(format!("node-{index}")))
        .collect();
    oversized["clusters"] = (0..count).map(|index| json!({"label":"Load","annotation":"Read the input.","nodes":[format!("node-{index}")]})).collect();
    let error = WorkflowDescription::from_json(&serde_json::to_vec(&oversized).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("too many node clusters"), "{error}");

    value["clusters"][0]["label"] = json!("");
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}
#[test]
fn nested_descriptions_validate_scope_ownership_and_version() {
    for source in ["%loop", "$loop"] {
        let mut value = serde_json::to_value(graph()).unwrap();
        value["version"] = json!("2026-09-29");
        value["nodes"] = json!([{"id": "repeat", "kind": "workflow.loop"}]);
        value["data_edges"] = json!([]);
        value["control_edges"] = json!([]);
        value["execution_order"] = json!(["repeat"]);
        value["loop_bodies"] = json!([{
            "path": ["repeat"],
            "source": source,
            "state_outputs": {"value": "value"},
            "nodes": [{"id": source, "kind": source}, {"id": "child", "kind": "fixture.echo"}],
            "data_edges": [{"from_node": source, "from_output": "value", "to_node": "child", "to_input": "input"}],
            "control_edges": [],
            "execution_order": [source, "child"]
        }]);
        let description =
            WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(description.loop_bodies[0].nodes[0].id, source);
        assert_eq!(description.workflow_id, graph().workflow_id);

        let mut renamed_source_kind = value.clone();
        renamed_source_kind["loop_bodies"][0]["nodes"][0]["kind"] = json!("fixture.source");
        WorkflowDescription::from_json(&serde_json::to_vec(&renamed_source_kind).unwrap()).unwrap();

        let mut invalid_source = value.clone();
        invalid_source["loop_bodies"][0]["source"] = json!("missing");
        assert!(
            WorkflowDescription::from_json(&serde_json::to_vec(&invalid_source).unwrap()).is_err()
        );

        value["loop_bodies"][0]["path"] = json!(["missing"]);
        assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
        value["loop_bodies"][0]["path"] = json!(["repeat"]);
        value["version"] = json!("2026-09-27");
        assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

#[test]
fn body_sources_are_explicit_endpoints_independent_of_instruction_labels() {
    for kind in ["workflow.loop", "builtin.iteration", "workflow.call"] {
        let value = json!({
            "version": "2026-10-23",
            "workflow_id": graph().workflow_id,
            "execution": {"mode": "single"},
            "nodes": [{"id": "owner", "kind": kind}],
            "data_edges": [], "control_edges": [], "execution_order": ["owner"],
            "loop_bodies": [{
                "path": ["owner"], "source": "renamed-input",
                "nodes": [
                    {"id": "renamed-input", "kind": "fixture.scope-input"},
                    {"id": "%loop", "kind": "fixture.authored"}
                ],
                "data_edges": [{
                    "from_node": "renamed-input", "from_output": "value",
                    "to_node": "%loop", "to_input": "input"
                }],
                "control_edges": [], "execution_order": ["renamed-input", "%loop"],
                "clusters": [{
                    "label": "Authored node", "annotation": "Not a synthetic endpoint.",
                    "nodes": ["%loop"]
                }]
            }]
        });
        let description =
            WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            description.loop_bodies[0].source.as_deref(),
            Some("renamed-input")
        );
        assert_eq!(
            WorkflowDescription::from_json(&description.to_json().unwrap()).unwrap(),
            description
        );

        let mut unknown = description.clone();
        unknown.loop_bodies[0].source = Some("missing".into());
        assert!(matches!(
            unknown.validate().unwrap_err(),
            mf_telemetry::ContractError::Invalid { message }
                if message == "unknown body source endpoint"
        ));

        let mut clustered_source = description.clone();
        clustered_source.loop_bodies[0].clusters[0].nodes = vec!["renamed-input".into()];
        assert!(clustered_source.validate().is_err());

        let mut duplicate = description.clone();
        let duplicate_node = duplicate.loop_bodies[0].nodes[0].clone();
        duplicate.loop_bodies[0].nodes.push(duplicate_node);
        assert!(duplicate.validate().is_err());

        // No relationship means there is no implicit source, even if an authored
        // node happens to use a historical synthetic ID or kind.
        let mut callable = description;
        callable.loop_bodies[0].source = None;
        callable.validate().unwrap();
        let encoded = serde_json::to_value(&callable).unwrap();
        assert!(encoded["loop_bodies"][0].get("source").is_none());
        assert_eq!(
            WorkflowDescription::from_json(&serde_json::to_vec(&encoded).unwrap()).unwrap(),
            callable
        );
    }
}

#[test]
fn scope_operators_follow_frame_relations_for_direct_and_encoded_body_keys() {
    let binding_key = mf_telemetry::event::ScopePathEntry::binding_key("owner", "body");
    for key in ["owner".to_owned(), binding_key] {
        for (source, state_outputs, accepts_loop, accepts_call) in [
            (Some("input"), json!({"state":"state"}), true, false),
            (Some("input"), json!({}), false, false),
            (None, json!({}), false, true),
        ] {
            let value = json!({
                "version":"2026-10-23", "workflow_id":graph().workflow_id,
                "execution":{"mode":"single"},
                "nodes":[{"id":"owner", "kind":"foreign.provider"}],
                "data_edges":[], "control_edges":[], "execution_order":["owner"],
                "loop_bodies":[{
                    "path":[key], "source":source, "state_outputs":state_outputs,
                    "nodes":[{"id":"input", "kind":"foreign.instruction"}],
                    "data_edges":[], "control_edges":[], "execution_order":["input"]
                }]
            });
            let description =
                WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
            let loop_path = vec![LoopPathEntry::Loop {
                loop_id: key.clone(),
                index: Count::ZERO,
            }];
            let call_path = vec![LoopPathEntry::Call {
                call_id: key.clone(),
            }];
            assert_eq!(
                description.validate_scope_path(&loop_path).is_ok(),
                accepts_loop
            );
            assert_eq!(
                description.validate_scope_path(&call_path).is_ok(),
                accepts_call
            );
            assert_eq!(
                description.loop_body(std::slice::from_ref(&key)).is_some(),
                accepts_loop || accepts_call
            );
            for (event, accepted) in [
                (
                    Event::LoopPassStarted {
                        path: loop_path,
                        elapsed_ns: Count::ZERO,
                    },
                    accepts_loop,
                ),
                (
                    Event::CallStarted {
                        path: call_path,
                        elapsed_ns: Count::ZERO,
                    },
                    accepts_call,
                ),
            ] {
                let event = LifecycleEvent {
                    workflow_id: description.workflow_id.clone(),
                    run_id: RunId::new(),
                    sequence: Count::try_from(2).unwrap(),
                    event,
                };
                assert_eq!(event.validate_for(&description).is_ok(), accepted);
            }
            assert_eq!(
                WorkflowDescription::from_json(&description.to_json().unwrap()).unwrap(),
                description
            );
        }
    }
}

#[test]
fn loop_summaries_require_owned_stateful_bodies_instead_of_provider_labels() {
    use mf_telemetry::event::{LoopStopReason, LoopSummary, NodeIdentity, Outcome};

    for kind in ["foreign.provider", "workflow.loop"] {
        for key in [
            "owner".to_owned(),
            LoopPathEntry::binding_key("owner", "body"),
        ] {
            for (source, state_outputs, accepted) in [
                (Some("renamed-input"), json!({"state":"state"}), true),
                (Some("renamed-input"), json!({}), false),
                (None, json!({}), false),
            ] {
                let description = WorkflowDescription::from_json(
                    &serde_json::to_vec(&json!({
                        "version":"2026-10-23", "workflow_id":graph().workflow_id,
                        "execution":{"mode":"single"},
                        "nodes":[{"id":"outer", "kind":"foreign.caller"}],
                        "execution_order":["outer"], "data_edges":[], "control_edges":[],
                        "loop_bodies":[{
                            "path":["outer"],
                            "nodes":[{"id":"owner", "kind":kind}, {"id":"other", "kind":kind}],
                            "execution_order":["owner", "other"], "data_edges":[], "control_edges":[]
                        }, {
                            "path":["outer", key], "source":source, "state_outputs":state_outputs,
                            "nodes":[{"id":"renamed-input", "kind":"foreign.input"}],
                            "execution_order":["renamed-input"], "data_edges":[], "control_edges":[]
                        }, {
                            "path":["outer", "other"], "source":"input",
                            "state_outputs":{"state":"state"},
                            "nodes":[{"id":"input", "kind":"foreign.input"}],
                            "execution_order":["input"], "data_edges":[], "control_edges":[]
                        }]
                    }))
                    .unwrap(),
                )
                .unwrap();
                let event = LifecycleEvent {
                    workflow_id: description.workflow_id.clone(),
                    run_id: RunId::new(),
                    sequence: Count::try_from(3).unwrap(),
                    event: Event::NodeFinished {
                        node: NodeIdentity {
                            id: "owner".into(),
                            kind: kind.into(),
                            path: vec![LoopPathEntry::Call {
                                call_id: "outer".into(),
                            }],
                        },
                        position: Count::ZERO,
                        elapsed_ns: Count::try_from(2).unwrap(),
                        duration_ns: Some(Count::try_from(1).unwrap()),
                        outcome: Outcome::Succeeded,
                        produced_ports: vec!["state".into()],
                        skipped_ports: Vec::new(),
                        failure: None,
                        loop_summary: Some(LoopSummary {
                            pass_count: Count::try_from(1).unwrap(),
                            reason: LoopStopReason::Maximum,
                        }),
                    },
                };
                // Wire validation has no description and cannot infer frame ownership.
                let wire = WireRecord::from_event(&event, 1, None).unwrap();
                let wire =
                    serde_json::from_slice::<WireRecord>(&serde_json::to_vec(&wire).unwrap())
                        .unwrap();
                let decoded = wire.decode().unwrap();
                assert_eq!(decoded, event);
                assert_eq!(decoded.validate_for(&description).is_ok(), accepted);

                let mut wrong_kind = event.clone();
                if let Event::NodeFinished { node, .. } = &mut wrong_kind.event {
                    node.kind = "different.provider".into();
                }
                assert!(wrong_kind.validate_for(&description).is_err());

                let mut excessive = event;
                if let Event::NodeFinished {
                    loop_summary: Some(summary),
                    ..
                } = &mut excessive.event
                {
                    summary.pass_count =
                        Count::try_from(i64::from(mf_telemetry::MAX_LOOP_ITERATIONS) + 1).unwrap();
                }
                assert!(excessive.event.validate().is_err());
            }
        }
    }
}

fn static_custom_children() -> WorkflowDescription {
    let key = mf_telemetry::event::ScopePathEntry::binding_key;
    // Encoded descriptor keys identify their physical owner independently of
    // unrelated authored IDs or presentation labels.
    let claimed = key("dispatch", "claimed");
    let body =
        |path: Vec<String>, id: &str, kind: &str, source: Option<&str>, state_outputs: Value| {
            json!({
                "path": path, "source": source, "state_outputs": state_outputs,
                "nodes": [{"id": id, "kind": kind}],
                "data_edges": [], "control_edges": [], "execution_order": [id]
            })
        };
    let value = json!({
        "version": "2026-09-29", "workflow_id": graph().workflow_id,
        "nodes": [
            {"id": "dispatch", "kind": "user.dispatch"},
            {"id": "repeat", "kind": "workflow.loop"},
            {"id": claimed, "kind": "workflow.loop"}
        ],
        "execution_order": ["dispatch", "repeat", claimed],
        "data_edges": [], "control_edges": [],
        "loop_bodies": [
            body(vec![key("dispatch", "selected")], "inner", "user.dispatch", None, json!({})),
            body(vec![key("dispatch", "fallback")], "inner", "fixture.fallback", None, json!({})),
            body(vec![key("dispatch", "selected"), key("inner", "selected")], "leaf", "fixture.leaf", None, json!({})),
            body(vec!["repeat".into()], "%loop", "%loop", Some("%loop"), json!({"value":"value"})),
            body(vec![claimed.clone()], "%loop", "%loop", Some("%loop"), json!({"value":"value"}))
        ]
    });
    WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn static_custom_child_lookup_respects_field_ownership_and_parent_scope() {
    let description = static_custom_children();
    let key = mf_telemetry::event::ScopePathEntry::binding_key;
    let selected = vec![key("dispatch", "selected")];
    let fallback = vec![key("dispatch", "fallback")];
    let nested = vec![key("dispatch", "selected"), key("inner", "selected")];
    let claimed = vec![key("dispatch", "claimed")];
    let repeat = vec!["repeat".to_owned()];
    for (parent, node, expected) in [
        (vec![], "dispatch", vec![&selected, &fallback, &claimed]),
        (selected.clone(), "inner", vec![&nested]),
        // Identical IDs in another child cannot claim this child's descendants.
        (fallback.clone(), "inner", vec![]),
        (vec!["missing".into()], "inner", vec![]),
        (vec![], "inner", vec![]),
        (vec![], claimed[0].as_str(), vec![]),
        (vec![], "repeat", vec![&repeat]),
    ] {
        assert_eq!(
            description.body_paths_for_node(&parent, node),
            expected,
            "{parent:?}/{node}"
        );
    }
    for path in [&selected, &fallback, &nested, &claimed, &repeat] {
        assert_eq!(description.loop_body(path), description.body(path));
        assert!(description.loop_body(path).is_some());
    }
    assert_eq!(
        description.loop_body(&selected).unwrap().nodes[0].kind,
        "user.dispatch"
    );
    assert_eq!(
        description.loop_body(&fallback).unwrap().nodes[0].kind,
        "fixture.fallback"
    );
    for missing in [
        vec![key("missing", "selected")],
        vec![key("dispatch", "not-prepared")],
        vec![fallback[0].clone(), nested[1].clone()],
    ] {
        assert!(description.loop_body(&missing).is_none(), "{missing:?}");
    }
    assert_eq!(
        WorkflowDescription::from_json(&description.to_json().unwrap()).unwrap(),
        description
    );
}

#[test]
fn static_custom_children_reject_corrupt_scope_facts_with_typed_errors() {
    let description = static_custom_children();
    let key = mf_telemetry::event::ScopePathEntry::binding_key;
    for (case, diagnostic) in [
        ("unknown_owner", "unknown body container"),
        ("missing_parent", "unknown parent Loop scope"),
        ("duplicate_scope", "duplicate Loop body path"),
        (
            "wrong_order",
            "execution order contains an unknown or duplicate node",
        ),
        ("unknown_source", "unknown body source endpoint"),
        (
            "state_without_source",
            "body state outputs require a source endpoint",
        ),
        ("invalid_state_output", "invalid body state output relation"),
    ] {
        let mut corrupt = description.clone();
        match case {
            "unknown_owner" => corrupt.loop_bodies[0].path = vec![key("missing", "selected")],
            "missing_parent" => {
                corrupt.loop_bodies.remove(0);
            }
            "duplicate_scope" => corrupt.loop_bodies.push(corrupt.loop_bodies[0].clone()),
            "wrong_order" => corrupt.loop_bodies[1].execution_order[0] = "leaf".into(),
            "unknown_source" => corrupt.loop_bodies[4].source = Some("missing".into()),
            "state_without_source" => corrupt.loop_bodies[4].source = None,
            "invalid_state_output" => {
                corrupt.loop_bodies[4]
                    .state_outputs
                    .insert("value".into(), "other".into());
            }
            _ => unreachable!(),
        }
        for error in [
            corrupt.validate().unwrap_err(),
            WorkflowDescription::from_json(&serde_json::to_vec(&corrupt).unwrap()).unwrap_err(),
        ] {
            assert!(
                matches!(&error, mf_telemetry::ContractError::Invalid { message }
                if message == diagnostic),
                "{case}: {error}"
            );
        }
    }
}

#[test]
fn additive_fields_are_ignored_but_required_fields_and_versions_are_checked() {
    let wire = record("success");
    let expected = wire.decode().unwrap();
    let mut extended = wire.clone();
    extended
        .attributes
        .insert("mf.future".into(), json!({"anything":true}));
    extended.body["future"] = json!([null, 1.5]);
    assert_eq!(expected, extended.decode().unwrap());
    for value in [json!(3), json!("1"), json!(1.0), json!(-1)] {
        let mut invalid = wire.clone();
        invalid.attributes.insert("mf.schema.version".into(), value);
        assert!(invalid.decode().is_err());
    }
    for key in [
        "mf.workflow.id",
        "mf.run.id",
        "mf.event.sequence",
        "mf.node.id",
        "mf.node.kind",
        "mf.outcome",
    ] {
        let mut invalid = wire.clone();
        invalid.attributes.remove(key);
        assert!(invalid.decode().is_err(), "{key}");
    }
    for value in [
        json!(-1),
        json!(1.5),
        json!(i64::MAX as u64 + 1),
        json!("10"),
        Value::Null,
    ] {
        let mut invalid = wire.clone();
        invalid.body["elapsed_ns"] = value;
        assert!(invalid.decode().is_err());
    }
    for value in [json!(0), json!(-1)] {
        let mut invalid = wire.clone();
        invalid.attributes.insert("mf.event.sequence".into(), value);
        assert!(invalid.decode().is_err());
    }
    let mut invalid = wire;
    invalid.body = json!("not a structured body");
    assert!(invalid.decode().is_err());
}

#[test]
fn terminal_contract_rejects_ambiguous_success_and_invalid_failure_context() {
    let cases = [
        ("success", "failure", json!({"message":"failure"})),
        ("success", "duration_ns", Value::Null),
        ("success", "duration_ns", json!(21)),
        ("success", "skipped_ports", json!(["value.part"])),
        (
            "success",
            "produced_ports",
            json!(["value.part", "value.part"]),
        ),
        ("failure", "duration_ns", json!(0)),
        ("failure", "produced_ports", json!(["value.part"])),
        ("skip", "causes", json!([])),
        ("start", "elapsed_ns", json!(1)),
    ];
    for (name, field, value) in cases {
        let mut invalid = record(name);
        invalid.body[field] = value;
        assert!(invalid.decode().is_err(), "{name}: {field}");
    }
    let mut invalid = record("failure");
    invalid
        .attributes
        .insert("mf.failure.phase".into(), json!("output_selection"));
    assert!(invalid.decode().is_err());
}

#[test]
fn dropped_reservations_leave_gaps_and_finish_closes_the_sequence() {
    let mut sequence = EventSequence::new();
    assert!(sequence.finish().is_err());
    assert_eq!(sequence.reserve().unwrap().get(), 1);
    let dropped = sequence.reserve().unwrap();
    assert_eq!(dropped.get(), 2);
    assert_eq!(sequence.reserve().unwrap().get(), 3);
    assert_eq!(sequence.finish().unwrap().get(), 4);
    assert!(sequence.reserve().is_err());
    assert!(sequence.finish().is_err());
    let mut empty = EventSequence::new();
    assert_eq!(empty.reserve().unwrap().get(), 1);
    assert_eq!(empty.finish().unwrap().get(), 2);
}

#[test]
fn description_validates_order_endpoints_and_opaque_edge_names() {
    let mut value = serde_json::to_value(graph()).unwrap();
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"target/one", "kind":"fixture.echo"}));
    value["execution_order"] = json!(["load.order", "target/one"]);
    value["data_edges"] = json!([{"from_node":"load.order","from_output":"value.part","to_node":"target/one","to_input":"input.name"}]);
    let decode =
        |value: &Value| WorkflowDescription::from_json(&serde_json::to_vec(value).unwrap());
    let valid = decode(&value).unwrap();
    assert_eq!(
        valid,
        WorkflowDescription::from_json(&valid.to_json().unwrap()).unwrap()
    );
    for (key, invalid_value) in [
        ("version", json!("2026-09-26")),
        ("execution_order", json!(["target/one", "load.order"])),
        ("execution_order", json!(["load.order"])),
        ("execution_order", json!(["load.order", "load.order"])),
        ("execution_order", json!(["load.order", "unknown"])),
    ] {
        let mut invalid = value.clone();
        invalid[key] = invalid_value;
        assert!(decode(&invalid).is_err());
    }
    let mut invalid = value.clone();
    invalid["data_edges"][0]["from_output"] = json!("");
    assert!(decode(&invalid).is_err());
    invalid = value.clone();
    invalid["data_edges"][0]["to_input"] = json!("");
    assert!(decode(&invalid).is_err());
    invalid = value.clone();
    invalid["data_edges"]
        .as_array_mut()
        .unwrap()
        .push(value["data_edges"][0].clone());
    assert!(decode(&invalid).is_err());
    value["future_field"] = json!({"nested":null});
    assert_eq!(valid, decode(&value).unwrap());
    assert!(WorkflowDescription::from_json(b"{} {}").is_err());
}

#[test]
fn graph_validation_checks_node_facts_without_bounding_transport_sequences() {
    let mut wrong_position = record("node-start");
    wrong_position.body["position"] = json!(1);
    assert!(
        wrong_position
            .decode()
            .unwrap()
            .validate_for(&graph())
            .is_err()
    );
    let mut wrong = record("node-start");
    wrong
        .attributes
        .insert("mf.node.kind".into(), json!("another.kind"));
    assert!(wrong.decode().unwrap().validate_for(&graph()).is_err());
    let mut unbounded = record("success");
    unbounded
        .attributes
        .insert("mf.event.sequence".into(), json!(i64::MAX));
    unbounded.decode().unwrap().validate_for(&graph()).unwrap();
    let mut wire = record("success");
    wire.body["produced_ports"] = json!(["unconnected.dynamic"]);
    wire.decode().unwrap().validate_for(&graph()).unwrap();
}

#[test]
fn graph_validation_accepts_unconnected_dynamic_port_names() {
    let mut wire = record("success");
    wire.body["produced_ports"] = json!(["unconnected.dynamic"]);
    assert!(wire.decode().unwrap().validate_for(&graph()).is_ok());
}

#[test]
fn trace_context_validates_native_ids_without_serializing_them_into_attributes() {
    let wire = record("node-start");
    let context = TraceContext {
        trace_id: "1234567890abcdef1234567890abcdef".into(),
        span_id: "1234567890abcdef".into(),
        trace_flags: 1,
    };
    let correlated = WireRecord::from_event(
        &wire.decode().unwrap(),
        wire.time_unix_nano,
        Some(context.clone()),
    )
    .unwrap();
    assert_eq!(wire.decode().unwrap(), correlated.decode().unwrap());
    assert!(!correlated.attributes.contains_key("trace_id"));
    for trace_id in [
        "00000000000000000000000000000000",
        "invalid",
        "1234567890ABCDEF1234567890ABCDEF",
    ] {
        let invalid = TraceContext {
            trace_id: trace_id.into(),
            ..context.clone()
        };
        assert!(WireRecord::from_event(&wire.decode().unwrap(), 0, Some(invalid)).is_err());
    }
}

#[test]
fn skip_metadata_matches_real_dependencies_without_a_full_port_table() {
    let graph = branch_graph();
    let skipped = record("skip");
    skipped.decode().unwrap().validate_for(&graph).unwrap();
    let mut router = record("success");
    router.body["skipped_ports"] = json!(["branch.false"]);
    router.decode().unwrap().validate_for(&graph).unwrap();
    router.body["produced_ports"] = json!([]);
    router.body["skipped_ports"] = json!(["value.part"]);
    router.decode().unwrap().validate_for(&graph).unwrap();
    let mut wrong = skipped.clone();
    wrong.body["skipped_ports"] = json!([]);
    wrong.decode().unwrap().validate_for(&graph).unwrap();
    wrong = skipped.clone();
    wrong.body["causes"][0]["source_output"] = json!("value.part");
    assert!(wrong.decode().unwrap().validate_for(&graph).is_err());
    let mut data_graph = serde_json::to_value(graph).unwrap();
    data_graph["control_edges"] = json!([]);
    data_graph["data_edges"] = json!([{"from_node":"load.order", "from_output":"branch.false", "to_node":"branch/target", "to_input":"input"}]);
    skipped
        .decode()
        .unwrap()
        .validate_for(
            &WorkflowDescription::from_json(&serde_json::to_vec(&data_graph).unwrap()).unwrap(),
        )
        .unwrap();
    let cause = wrong.body["causes"][0].clone();
    wrong.body["causes"].as_array_mut().unwrap().push(cause);
    assert!(wrong.decode().is_err());
}

#[test]
fn all_failure_phases_preserve_node_identity_without_visit_accounting() {
    for phase in ["preparation", "dependency", "execution", "publication"] {
        let mut node = record("failure");
        node.attributes
            .insert("mf.failure.phase".into(), json!(phase));
        if matches!(phase, "execution" | "publication") {
            node.body["duration_ns"] = json!(5);
        }
        node.decode().unwrap().validate_for(&graph()).unwrap();
        let mut finish = record("failed-finish");
        finish
            .attributes
            .insert("mf.failure.phase".into(), json!(phase));
        finish
            .decode()
            .unwrap()
            .validate_for(&branch_graph())
            .unwrap();
        finish.body["failure_node_id"] = json!("unknown.node");
        assert!(
            finish
                .decode()
                .unwrap()
                .validate_for(&branch_graph())
                .is_err()
        );
        if phase != "preparation" {
            finish
                .body
                .as_object_mut()
                .unwrap()
                .remove("failure_node_id");
            assert!(finish.decode().is_err());
        }
    }
    let mut output_failure = record("failed-finish");
    output_failure
        .attributes
        .insert("mf.failure.phase".into(), json!("output_selection"));
    output_failure
        .body
        .as_object_mut()
        .unwrap()
        .remove("failure_node_id");
    output_failure
        .decode()
        .unwrap()
        .validate_for(&graph())
        .unwrap();
    output_failure.body["failure"]["message"] = json!("");
    assert!(output_failure.decode().is_err());
}

#[test]
fn descriptions_reject_duplicate_nodes_and_control_edges() {
    let original = serde_json::to_value(branch_graph()).unwrap();
    for path in ["nodes", "control_edges"] {
        let mut value = original.clone();
        let duplicate = value[path][0].clone();
        value[path].as_array_mut().unwrap().push(duplicate);
        assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = original.clone();
    value["nodes"][0]["id"] = json!(" ");
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = original.clone();
    value["control_edges"][0]["from_output"] = json!("");
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    value = original;
    value["control_edges"][0]["from_node"] = json!("unknown");
    assert!(WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    let oversized = vec![b' '; mf_telemetry::description::MAX_DESCRIPTION_BYTES + 1];
    assert!(WorkflowDescription::from_json(&oversized).is_err());
}

#[test]
fn rejects_misplaced_routing_fields_and_failure_context() {
    let mut cases = Vec::new();
    let mut wire = record("start");
    wire.scope = "another.scope".into();
    cases.push(wire);
    let mut wire = record("start");
    wire.attributes
        .insert("mf.node.id".into(), json!("load.order"));
    cases.push(wire);
    let mut wire = record("node-start");
    wire.body["outcome"] = json!("succeeded");
    cases.push(wire);
    let mut wire = record("skip");
    wire.attributes.insert("mf.outcome".into(), json!("failed"));
    cases.push(wire);
    let mut wire = record("start");
    wire.attributes
        .insert("mf.outcome".into(), json!("succeeded"));
    cases.push(wire);
    let mut wire = record("failure");
    wire.body["failure"] = json!("a string");
    cases.push(wire);
    let mut wire = record("failure");
    wire.body["failure"]["phase"] = json!("preparation");
    cases.push(wire);
    let mut wire = record("node-start");
    wire.attributes
        .insert("mf.failure.phase".into(), json!("preparation"));
    cases.push(wire.clone());
    wire.body["failure"] = json!({"message":"failed"});
    cases.push(wire);
    for wire in cases {
        assert!(wire.decode().is_err(), "{}", wire.event_name);
    }
}

#[cfg(feature = "otlp")]
#[test]
fn fills_real_otel_records_with_structured_bodies_and_signed_integers() {
    use opentelemetry::{
        Key,
        logs::{AnyValue, Logger, LoggerProvider},
    };
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    let provider = SdkLoggerProvider::builder().build();
    let logger = provider.logger(mf_telemetry::INSTRUMENTATION_SCOPE);
    for name in [
        "start",
        "node-start",
        "success",
        "finish",
        "failure",
        "failed-finish",
        "skip",
    ] {
        let wire = record(name);
        let mut log = logger.create_log_record();
        wire.write_to(&mut log).unwrap();
        assert_eq!(log.event_name(), Some(wire.decode().unwrap().event.name()));
        assert!(matches!(log.body(), Some(AnyValue::Map(_))));
        assert!(log.attributes_iter().any(|(key, value)| key
            == &Key::from_static_str("mf.event.sequence")
            && matches!(value, AnyValue::Int(_))));
        assert_eq!(
            log.timestamp()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            u128::from(wire.time_unix_nano)
        );
    }
    let mut wire = record("node-start");
    wire.trace_context = Some(TraceContext {
        trace_id: "1234567890abcdef1234567890abcdef".into(),
        span_id: "1234567890abcdef".into(),
        trace_flags: 1,
    });
    let mut log = logger.create_log_record();
    wire.write_to(&mut log).unwrap();
    assert_eq!(
        log.trace_context().unwrap().trace_id.to_string(),
        "1234567890abcdef1234567890abcdef"
    );
    assert_eq!(
        log.trace_context().unwrap().span_id.to_string(),
        "1234567890abcdef"
    );
    provider.shutdown().unwrap();
}

#[test]
fn identifier_parsing_preserves_typed_sources() {
    use std::error::Error;
    let error = RunId::try_from("not-a-uuid".to_owned()).unwrap_err();
    assert!(error.source().unwrap().is::<uuid::Error>());
    let mut context = TraceContext {
        trace_id: "invalid".into(),
        span_id: "0123456789abcdef".into(),
        trace_flags: 1,
    };
    let error = context.validate().unwrap_err();
    assert!(matches!(error, mf_telemetry::ContractError::TraceId { .. }));
    assert!(error.source().unwrap().is::<std::num::ParseIntError>());
    context.trace_id = "0123456789abcdef0123456789abcdef".into();
    context.span_id = "invalid".into();
    let error = context.validate().unwrap_err();
    assert!(matches!(error, mf_telemetry::ContractError::SpanId { .. }));
    assert!(error.source().unwrap().is::<std::num::ParseIntError>());
}

#[test]
fn selected_inputs_round_trip_and_require_declared_exclusive_sources() {
    let mut description = serde_json::to_value(branch_graph()).unwrap();
    description["data_edges"] = json!([
        {"from_node":"load.order", "from_output":"first", "to_node":"branch/target", "to_input":"value", "exclusive":true},
        {"from_node":"load.order", "from_output":"second", "to_node":"branch/target", "to_input":"value", "exclusive":true}
    ]);
    let description =
        WorkflowDescription::from_json(&serde_json::to_vec(&description).unwrap()).unwrap();
    let mut wire = record("skip");
    wire.body["selected_inputs"] =
        json!([{"input":"value", "source_node":"load.order", "source_output":"first"}]);
    let decoded = wire.decode().unwrap();
    decoded.validate_for(&description).unwrap();
    assert_eq!(decoded.event.selected_inputs().len(), 1);
    wire.body["selected_inputs"][0]["source_output"] = json!("unknown");
    assert!(wire.decode().unwrap().validate_for(&description).is_err());
    wire.body["selected_inputs"][0]["source_output"] = json!("");
    assert!(wire.decode().is_err());
}

#[cfg(feature = "otlp")]
#[test]
fn observations_share_the_original_timestamp_between_local_and_otel_consumers() {
    use mf_telemetry::events;
    use opentelemetry::logs::{AnyValue, Logger, LoggerProvider};
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use std::sync::{Arc, Mutex};
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let received = recorded.clone();
    let _events = events::collect(move |event| received.lock().unwrap().push(event.clone()));
    let trace =
        opentelemetry::trace::TraceId::from_hex("1234567890abcdef1234567890abcdef").unwrap();
    let span = opentelemetry::trace::SpanId::from_hex("1234567890abcdef").unwrap();
    use opentelemetry::trace::{SpanContext, TraceContextExt, TraceState};
    let _context = opentelemetry::Context::current()
        .with_remote_span_context(SpanContext::new(
            trace,
            span,
            opentelemetry::TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        ))
        .attach();
    events::record(
        "llm.request.started",
        json!({"call_id":"request","model":"fixture"}),
    )
    .unwrap();
    let event = recorded.lock().unwrap()[0].clone();
    let provider = SdkLoggerProvider::builder().build();
    let logger = provider.logger(mf_telemetry::events::SCOPE);
    let mut log = logger.create_log_record();
    event
        .write_to(
            "llm.request.started",
            &graph().workflow_id,
            RunId::new(),
            &mut log,
        )
        .unwrap();
    assert_eq!(log.event_name(), Some("llm.request.started"));
    assert_eq!(
        log.timestamp()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        u128::from(recorded.lock().unwrap()[0].time_unix_nano)
    );
    assert!(matches!(log.body(), Some(AnyValue::Map(_))));
    assert_eq!(
        log.trace_context().unwrap().span_id.to_string(),
        "1234567890abcdef"
    );
    assert!(
        event
            .write_to("other.event", &graph().workflow_id, RunId::new(), &mut log)
            .is_err()
    );
}

#[test]
fn observation_sources_capture_the_active_trace_identity() {
    use opentelemetry::{
        Context, TraceFlags,
        trace::{SpanContext, SpanId, TraceContextExt, TraceId, TraceState},
    };
    let trace = TraceId::from_bytes([1; 16]);
    let span = SpanId::from_bytes([2; 8]);
    let correlation = SpanContext::new(
        trace,
        span,
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    );
    let _context = Context::new()
        .with_remote_span_context(correlation)
        .attach();
    let record =
        mf_telemetry::events::EventRecord::now("fixture.event", json!({"call_id":"call"})).unwrap();
    let captured = record.trace_context.unwrap();
    assert_eq!(captured.trace_id, trace.to_string());
    assert_eq!(captured.span_id, span.to_string());
    assert_eq!(captured.trace_flags, 1);
    assert!(record.time_unix_nano > 0);
}
