use mf_telemetry::{
    Count, MAX_SCOPE_DEPTH,
    description::{WorkflowDescription, WorkflowDescriptionVersion},
    event::{Event, LifecycleEvent, LoopPassOutcome, ScopePathEntry},
    identity::RunId,
    stream::{StreamIdentity, StreamMessage, StreamPayload, StreamRecord, StreamTrigger},
    wire::WireRecord,
};
use serde_json::json;

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

type ScopeFixture<'a> = (&'a str, Option<&'a str>, &'a [(&'a str, &'a str)]);

const LOOP: ScopeFixture<'static> = ("workflow.loop", Some("%loop"), &[("value", "value")]);
const CALL: ScopeFixture<'static> = ("workflow.call", None, &[]);

fn graph(scopes: &[ScopeFixture<'_>]) -> WorkflowDescription {
    let mut bodies = Vec::new();
    for (index, (_, source, state_outputs)) in scopes.iter().enumerate() {
        let mut nodes = Vec::new();
        if let Some(source) = source {
            nodes.push(json!({"id":source, "kind":"fixture.scope-input"}));
        }
        nodes.push(if let Some((kind, _, _)) = scopes.get(index + 1) {
            json!({"id":format!("scope{}", index + 1), "kind":kind})
        } else {
            json!({"id":"leaf", "kind":"fixture.task"})
        });
        bodies.push(json!({
            "path":(0..=index).map(|index| format!("scope{index}")).collect::<Vec<_>>(),
            "execution_order":nodes.iter().map(|node| node["id"].clone()).collect::<Vec<_>>(),
            "source":source,
            "state_outputs":state_outputs.iter().copied().collect::<std::collections::BTreeMap<_, _>>(),
            "nodes":nodes, "data_edges":[], "control_edges":[]
        }));
    }
    serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "a".repeat(64)),
        "execution":{"mode":"single"},
        "nodes":[{"id":"scope0", "kind":scopes[0].0}],
        "execution_order":["scope0"], "data_edges":[], "control_edges":[],
        "loop_bodies":bodies
    }))
    .unwrap()
}

fn call_path() -> Vec<ScopePathEntry> {
    vec![ScopePathEntry::Call {
        call_id: "scope0".into(),
    }]
}

fn lifecycle(event: Event, graph: &WorkflowDescription) -> LifecycleEvent {
    LifecycleEvent {
        workflow_id: graph.workflow_id.clone(),
        run_id: RunId::new(),
        sequence: count(2),
        event,
    }
}

#[test]
fn loop_wire_shape_is_preserved_and_scope_tags_are_strict() {
    let legacy = json!([{"loop_id":"scope0", "index":3}]);
    let path: Vec<ScopePathEntry> = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(&path).unwrap(), legacy);
    assert_eq!(
        serde_json::to_value(call_path()).unwrap(),
        json!([{"call_id":"scope0"}])
    );
    for invalid in [
        json!({"call_id":"scope0", "index":0}),
        json!({"loop_id":"scope0", "call_id":"scope0", "index":0}),
        json!({"loop_id":"scope0"}),
        json!({"loop_id":"scope0", "index":-1}),
    ] {
        assert!(serde_json::from_value::<ScopePathEntry>(invalid).is_err());
    }
}

#[test]
fn call_lifecycles_round_trip_in_finite_and_stream_wire_records() {
    let graph = graph(&[CALL]);
    graph.validate().unwrap();
    for event in [
        Event::CallStarted {
            path: call_path(),
            elapsed_ns: count(1),
        },
        Event::CallFinished {
            path: call_path(),
            elapsed_ns: count(2),
            outcome: LoopPassOutcome::Completed,
        },
        Event::CallFinished {
            path: call_path(),
            elapsed_ns: count(2),
            outcome: LoopPassOutcome::Failed,
        },
    ] {
        let record = lifecycle(event.clone(), &graph);
        record.validate_for(&graph).unwrap();
        let wire = WireRecord::from_event(&record, 0, None).unwrap();
        assert_eq!(wire.decode().unwrap(), record);
        assert_eq!(wire.event_name, event.name());
        let stream = StreamRecord {
            workflow_id: graph.workflow_id.clone(),
            run_id: record.run_id,
            sequence: count(2),
            emission_count: None,
            identity: Some(StreamIdentity {
                invocation: 1,
                parent: None,
                trigger: StreamTrigger::Message,
                message: Some(StreamMessage {
                    domain: 1,
                    sequence: 0,
                }),
            }),
            payload: StreamPayload::Execution(event),
        };
        assert_eq!(
            StreamRecord::decode(&stream.to_wire(0, None).unwrap()).unwrap(),
            stream
        );
    }
    for version in [
        WorkflowDescriptionVersion::V2026_09_29,
        WorkflowDescriptionVersion::V2026_10_03,
        WorkflowDescriptionVersion::V2026_10_19,
        WorkflowDescriptionVersion::V2026_10_23,
    ] {
        let mut existing = graph.clone();
        existing.version = version;
        if version == WorkflowDescriptionVersion::V2026_09_29 {
            existing.execution = None;
        }
        existing.validate().unwrap();
        let event = lifecycle(
            Event::CallStarted {
                path: call_path(),
                elapsed_ns: Count::ZERO,
            },
            &existing,
        );
        event.validate_for(&existing).unwrap();
    }
    let mut legacy = graph.clone();
    legacy.version = WorkflowDescriptionVersion::V2026_09_27;
    legacy.execution = None;
    assert!(legacy.validate().is_err());
    assert!(legacy.validate_scope_path(&call_path()).is_err());
}

#[test]
fn call_depth_is_independent_of_versions_and_preserves_loop_only_limits() {
    let path = |depth: usize| {
        (0..depth)
            .map(|index| format!("scope{index}"))
            .collect::<Vec<_>>()
    };
    let loops = graph(&[LOOP; mf_telemetry::MAX_LOOP_DEPTH]);
    loops.validate().unwrap();
    assert!(
        loops
            .loop_body(&path(mf_telemetry::MAX_LOOP_DEPTH))
            .is_some()
    );
    let excessive = graph(&[LOOP; mf_telemetry::MAX_LOOP_DEPTH + 1]);
    assert!(excessive.validate().is_err());
    assert!(
        excessive
            .loop_body(&path(mf_telemetry::MAX_LOOP_DEPTH + 1))
            .is_none()
    );
    let mut scopes = vec![LOOP; mf_telemetry::MAX_LOOP_DEPTH];
    scopes.push(CALL);
    scopes.extend([LOOP; mf_telemetry::MAX_LOOP_DEPTH]);
    let mixed = graph(&scopes);
    mixed.validate().unwrap();
    assert!(mixed.loop_body(&path(scopes.len())).is_some());
    let calls = graph(&[CALL; MAX_SCOPE_DEPTH]);
    calls.validate().unwrap();
    assert!(calls.loop_body(&path(MAX_SCOPE_DEPTH)).is_some());
    assert!(calls.loop_body(&[]).is_none());
    assert!(calls.loop_body(&path(MAX_SCOPE_DEPTH + 1)).is_none());
    assert!(graph(&[CALL; MAX_SCOPE_DEPTH + 1]).validate().is_err());
    let event = Event::CallStarted {
        path: vec![call_path()[0].clone(); MAX_SCOPE_DEPTH + 1],
        elapsed_ns: Count::ZERO,
    };
    assert!(lifecycle(event, &calls).validate().is_err());
    let mut ordinary = graph(&[("fixture.task", None, &[])]);
    ordinary.validate().unwrap();
    assert!(ordinary.loop_body(&path(1)).is_some());
    ordinary.loop_bodies.clear();
    assert!(ordinary.loop_body(&path(1)).is_none());
}

#[test]
fn scope_paths_check_frame_relations() {
    let graph = graph(&[LOOP, CALL, LOOP]);
    graph.validate().unwrap();
    let path = vec![
        ScopePathEntry::Loop {
            loop_id: "scope0".into(),
            index: count(2),
        },
        ScopePathEntry::Call {
            call_id: "scope1".into(),
        },
        ScopePathEntry::Loop {
            loop_id: "scope2".into(),
            index: count(3),
        },
    ];
    graph.validate_scope_path(&path).unwrap();
    assert!(
        graph
            .loop_body(&["scope0".into(), "scope1".into(), "scope2".into()])
            .is_some()
    );
    for invalid in [
        call_path(),
        vec![
            path[0].clone(),
            ScopePathEntry::Loop {
                loop_id: "scope1".into(),
                index: Count::ZERO,
            },
        ],
        vec![ScopePathEntry::Call {
            call_id: "unknown".into(),
        }],
    ] {
        assert!(graph.validate_scope_path(&invalid).is_err());
    }
}

#[test]
fn top_level_flow_binding_lifecycles_round_trip_and_match_description_paths() {
    for binding in ["first", "second", "body"] {
        let key = ScopePathEntry::binding_key("scope0", binding);
        let path = vec![ScopePathEntry::Call {
            call_id: key.clone(),
        }];
        let mut value = serde_json::to_value(graph(&[CALL])).unwrap();
        value["nodes"][0]["kind"] = json!("fixture.custom");
        value["loop_bodies"][0]["path"] = json!([key]);
        let graph = WorkflowDescription::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        graph.validate_scope_path(&path).unwrap();
        assert!(graph.loop_body(std::slice::from_ref(&key)).is_some());
        for event in [
            Event::CallStarted {
                path: path.clone(),
                elapsed_ns: count(1),
            },
            Event::CallFinished {
                path: path.clone(),
                elapsed_ns: count(2),
                outcome: LoopPassOutcome::Completed,
            },
        ] {
            let record = lifecycle(event, &graph);
            record.validate_for(&graph).unwrap();
            let wire = WireRecord::from_event(&record, 0, None).unwrap();
            assert_eq!(wire.body["path"][0], json!({"call_id":key}));
            let decoded = wire.decode().unwrap();
            assert_eq!(decoded, record);
            decoded.validate_for(&graph).unwrap();
            let stream = StreamRecord {
                workflow_id: graph.workflow_id.clone(),
                run_id: record.run_id,
                sequence: record.sequence,
                emission_count: None,
                identity: Some(StreamIdentity {
                    invocation: 1,
                    parent: None,
                    trigger: StreamTrigger::Startup,
                    message: None,
                }),
                payload: StreamPayload::Execution(record.event),
            };
            let decoded = StreamRecord::decode(&stream.to_wire(0, None).unwrap()).unwrap();
            assert_eq!(decoded, stream);
        }
    }
}

#[test]
fn call_events_reject_loop_semantics_and_invalid_paths() {
    let graph = graph(&[CALL]);
    for path in [
        Vec::new(),
        vec![ScopePathEntry::Call {
            call_id: " ".into(),
        }],
        vec![ScopePathEntry::Loop {
            loop_id: "scope0".into(),
            index: Count::ZERO,
        }],
    ] {
        assert!(
            lifecycle(
                Event::CallStarted {
                    path,
                    elapsed_ns: Count::ZERO
                },
                &graph
            )
            .validate()
            .is_err()
        );
    }
    let event = lifecycle(
        Event::CallFinished {
            path: call_path(),
            elapsed_ns: Count::ZERO,
            outcome: LoopPassOutcome::Exit,
        },
        &graph,
    );
    assert!(event.validate_for(&graph).is_err());
    for event in [
        Event::LoopPassStarted {
            path: call_path(),
            elapsed_ns: Count::ZERO,
        },
        Event::LoopPassFinished {
            path: call_path(),
            elapsed_ns: Count::ZERO,
            outcome: LoopPassOutcome::Completed,
        },
    ] {
        assert!(lifecycle(event, &graph).validate().is_err());
    }
}
