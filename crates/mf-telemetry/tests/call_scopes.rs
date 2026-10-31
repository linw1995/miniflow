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

fn graph(kinds: &[&str]) -> WorkflowDescription {
    let mut bodies = Vec::new();
    for (index, kind) in kinds.iter().enumerate() {
        let mut nodes = Vec::new();
        if *kind == "workflow.loop" {
            nodes.push(json!({"id":"%loop", "kind":"%loop"}));
        }
        nodes.push(if let Some(next) = kinds.get(index + 1) {
            json!({"id":format!("scope{}", index + 1), "kind":next})
        } else {
            json!({"id":"leaf", "kind":"fixture.task"})
        });
        bodies.push(json!({
            "path":(0..=index).map(|index| format!("scope{index}")).collect::<Vec<_>>(),
            "execution_order":nodes.iter().map(|node| node["id"].clone()).collect::<Vec<_>>(),
            "nodes":nodes, "data_edges":[], "control_edges":[]
        }));
    }
    serde_json::from_value(json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "a".repeat(64)),
        "execution":{"mode":"single"},
        "nodes":[{"id":"scope0", "kind":kinds[0]}],
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
    let graph = graph(&["workflow.call"]);
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
    let loops = graph(&["workflow.loop"; mf_telemetry::MAX_LOOP_DEPTH]);
    loops.validate().unwrap();
    assert!(
        loops
            .loop_body(&path(mf_telemetry::MAX_LOOP_DEPTH))
            .is_some()
    );
    let excessive = graph(&["workflow.loop"; mf_telemetry::MAX_LOOP_DEPTH + 1]);
    assert!(excessive.validate().is_err());
    assert!(
        excessive
            .loop_body(&path(mf_telemetry::MAX_LOOP_DEPTH + 1))
            .is_none()
    );
    let mut kinds = vec!["workflow.loop"; mf_telemetry::MAX_LOOP_DEPTH];
    kinds.push("workflow.call");
    kinds.extend(["workflow.loop"; mf_telemetry::MAX_LOOP_DEPTH]);
    let mixed = graph(&kinds);
    mixed.validate().unwrap();
    assert!(mixed.loop_body(&path(kinds.len())).is_some());
    let calls = graph(&["workflow.call"; MAX_SCOPE_DEPTH]);
    calls.validate().unwrap();
    assert!(calls.loop_body(&path(MAX_SCOPE_DEPTH)).is_some());
    assert!(calls.loop_body(&[]).is_none());
    assert!(calls.loop_body(&path(MAX_SCOPE_DEPTH + 1)).is_none());
    assert!(
        graph(&["workflow.call"; MAX_SCOPE_DEPTH + 1])
            .validate()
            .is_err()
    );
    let event = Event::CallStarted {
        path: vec![call_path()[0].clone(); MAX_SCOPE_DEPTH + 1],
        elapsed_ns: Count::ZERO,
    };
    assert!(lifecycle(event, &calls).validate().is_err());
    let mut ordinary = graph(&["fixture.task"]);
    assert!(ordinary.validate().is_err());
    assert!(ordinary.loop_body(&path(1)).is_none());
    ordinary.loop_bodies.clear();
    assert!(ordinary.loop_body(&path(1)).is_none());
}

#[test]
fn scope_paths_check_container_kinds() {
    let graph = graph(&["workflow.loop", "workflow.call", "workflow.loop"]);
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
fn call_events_reject_loop_semantics_and_invalid_paths() {
    let graph = graph(&["workflow.call"]);
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

#[test]
fn generic_parameter_containers_use_declared_call_scopes_in_the_new_description() {
    let mut description = graph(&["fixture.container", "workflow.call", "workflow.loop"]);
    assert!(description.validate().is_err());
    description.version = WorkflowDescriptionVersion::V2026_10_31;
    description.validate().unwrap();
    let path = vec![
        ScopePathEntry::Call {
            call_id: "scope0".into(),
        },
        ScopePathEntry::Call {
            call_id: "scope1".into(),
        },
        ScopePathEntry::Loop {
            loop_id: "scope2".into(),
            index: count(0),
        },
    ];
    description.validate_scope_path(&path).unwrap();
    assert!(
        description
            .loop_body(&["scope0".into(), "scope1".into(), "scope2".into()])
            .is_some()
    );
    assert_eq!(
        description.scope_node(&["scope0".into()]).unwrap().kind,
        "fixture.container"
    );
    assert!(!description.is_call_scope(&["scope0".into(), "scope1".into(), "scope2".into()]));
    let call = lifecycle(
        Event::CallStarted {
            path: call_path(),
            elapsed_ns: count(1),
        },
        &description,
    );
    call.validate_for(&description).unwrap();
    let mut missing = description.clone();
    missing.loop_bodies.clear();
    assert!(!missing.is_call_scope(&["scope0".into()]));
    assert!(missing.validate_scope_path(&call_path()).is_err());
}
