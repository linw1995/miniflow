use mf_telemetry::{
    Count,
    description::WorkflowDescription,
    event::{Event, EventSequence, LifecycleEvent, LoopPathEntry},
    identity::{RunId, WorkflowId},
    maximum_event_count,
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
fn loop_pass_wire_records_require_the_new_schema() {
    let event = LifecycleEvent {
        workflow_id: graph().workflow_id,
        run_id: RunId::new(),
        sequence: Count::try_from(2).unwrap(),
        event: Event::LoopPassStarted {
            path: vec![LoopPathEntry {
                loop_id: "repeat".into(),
                index: Count::ZERO,
            }],
            elapsed_ns: Count::try_from(1).unwrap(),
        },
    };
    assert!(WireRecord::from_event(&event, 1, None).is_err());
    let wire = WireRecord::from_event_with_version(&event, 2, 1, None).unwrap();
    assert_eq!(wire.schema_version().unwrap(), 2);
    assert_eq!(wire.decode().unwrap(), event);
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
            "nodes": [{"id": source, "kind": source}, {"id": "child", "kind": "fixture.echo"}],
            "data_edges": [{"from_node": source, "from_output": "value", "to_node": "child", "to_input": "input"}],
            "control_edges": [],
            "execution_order": [source, "child"]
        }]);
        let description =
            WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(description.loop_bodies[0].nodes[0].id, source);
        assert_eq!(description.workflow_id, graph().workflow_id);

        let mut invalid_source = value.clone();
        invalid_source["loop_bodies"][0]["nodes"][0]["kind"] = json!("fixture.source");
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
        ("finish", "final_sequence", json!(99)),
        ("failed-finish", "visited_node_count", json!(1)),
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
    let mut sequence = EventSequence::new(Count::try_from(1).unwrap()).unwrap();
    assert!(sequence.finish().is_err());
    assert_eq!(sequence.reserve().unwrap().get(), 1);
    let dropped = sequence.reserve().unwrap();
    assert_eq!(dropped.get(), 2);
    assert_eq!(sequence.reserve().unwrap().get(), 3);
    assert!(sequence.reserve().is_err());
    assert_eq!(sequence.finish().unwrap().get(), 4);
    assert!(sequence.reserve().is_err());
    assert!(sequence.finish().is_err());
    assert!(maximum_event_count(Count::try_from(i64::MAX).unwrap()).is_err());
    let mut empty = EventSequence::new(Count::ZERO).unwrap();
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
fn graph_relative_event_validation_rejects_wrong_nodes_and_out_of_range_evidence() {
    for (name, field, value) in [
        ("node-start", "position", json!(1)),
        ("start", "node_count", json!(2)),
        ("finish", "visited_node_count", json!(0)),
        ("finish", "visited_node_count", json!(2)),
    ] {
        let mut wire = record(name);
        wire.body[field] = value;
        assert!(wire.decode().unwrap().validate_for(&graph()).is_err());
    }
    let mut wrong = record("node-start");
    wrong
        .attributes
        .insert("mf.node.kind".into(), json!("another.kind"));
    assert!(wrong.decode().unwrap().validate_for(&graph()).is_err());
    let mut wrong = record("success");
    wrong
        .attributes
        .insert("mf.event.sequence".into(), json!(i64::MAX));
    assert!(wrong.decode().unwrap().validate_for(&graph()).is_err());
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
fn all_failure_phases_preserve_invocation_and_visited_prefix_boundaries() {
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
        finish.body["visited_node_count"] = json!(if phase == "preparation" { 0 } else { 1 });
        finish
            .decode()
            .unwrap()
            .validate_for(&branch_graph())
            .unwrap();
        if phase != "preparation" {
            finish.body["failure_node_id"] = json!("branch/target");
            assert!(
                finish
                    .decode()
                    .unwrap()
                    .validate_for(&branch_graph())
                    .is_err()
            );
            finish.body["visited_node_count"] = json!(0);
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
    assert!(
        output_failure
            .decode()
            .unwrap()
            .validate_for(&graph())
            .is_err()
    );
    output_failure.body["visited_node_count"] = json!(1);
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
