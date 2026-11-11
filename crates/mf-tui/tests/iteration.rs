use mf_telemetry::{
    Count,
    description::WorkflowDescription,
    event::{Event, Failure, FailurePhase, LifecycleEvent, NodeIdentity, Outcome},
    identity::RunId,
    iteration::IterationContext,
};
use mf_tui::state::{Admission, IterationEvent, IterationRecord, NodeStatus, SessionState};

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

fn graph() -> WorkflowDescription {
    serde_json::from_value(serde_json::json!({
        "version":"2026-10-23", "workflow_id":format!("sha256:{}", "b".repeat(64)),
        "execution":{"mode":"single"},
        "nodes":[{"id":"each","kind":"builtin.iteration"},{"id":"other","kind":"builtin.iteration"}],
        "execution_order":["each","other"], "data_edges":[],"control_edges":[],
        "loop_bodies": (["each", "other"].map(|id| serde_json::json!({
            "path":[id], "source":"%iteration", "nodes":[{"id":"%iteration","kind":"builtin.iteration_input"},{"id":"work","kind":"fixture.work"}],
            "execution_order":["%iteration","work"],"data_edges":[],"control_edges":[]
        })))
    })).unwrap()
}

fn record(invocation: i64, index: i64, sequence: i64, event: IterationEvent) -> IterationRecord {
    IterationRecord {
        context: IterationContext {
            invocation: count(invocation),
            iteration_path: vec!["each".into()],
            body_path: vec!["each".into()],
            parent_path: Vec::new(),
            parent_stream_invocation: None,
            index: count(index),
            sequence: count(sequence),
            elapsed_ns: count(invocation * 100_000 + sequence),
        },
        event,
        correlation: Some((vec![1; 16], vec![index as u8 + 1; 8])),
    }
}

fn started() -> IterationEvent {
    IterationEvent::NodeStarted {
        node: NodeIdentity {
            id: "work".into(),
            kind: "fixture.work".into(),
            path: Vec::new(),
        },
        selected_inputs: Vec::new(),
    }
}
fn finished(outcome: Outcome) -> IterationEvent {
    IterationEvent::NodeFinished {
        node: NodeIdentity {
            id: "work".into(),
            kind: "fixture.work".into(),
            path: Vec::new(),
        },
        outcome,
        duration_ns: Some(count(7)),
        produced_ports: vec!["value".into()],
        skipped_ports: Vec::new(),
        failure: (outcome == Outcome::Failed).then(|| Failure {
            phase: FailurePhase::Execution,
            message: "fixture failure".into(),
        }),
    }
}

#[test]
fn replay_marks_only_the_explicit_scope_source_as_succeeded() {
    for source in [Some("renamed-input".to_owned()), None] {
        let mut description = graph();
        let body = &mut description.loop_bodies[0];
        body.source = source.clone();
        body.nodes[0].id = "renamed-input".into();
        body.nodes[0].kind = "fixture.scope-input".into();
        body.execution_order[0] = "renamed-input".into();
        // A presentation label alone must not synthesize an execution result.
        body.nodes[1].kind = "builtin.iteration_input".into();
        let mut state = SessionState::new(description, RunId::new()).unwrap();
        let admission = state.apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted));
        if source.is_none() {
            assert!(admission.is_err());
            continue;
        }
        admission.unwrap();
        let snapshot = state.snapshot();
        let nodes = snapshot.iterations.items[0].display_nodes(
            &["each".into()],
            [
                ("renamed-input", "fixture.scope-input"),
                ("work", "builtin.iteration_input"),
            ],
        );
        assert_eq!(nodes[0].status, NodeStatus::Succeeded);
        assert_eq!(nodes[1].status, NodeStatus::Pending);
        assert_eq!(nodes[0].produced_ports, ["item", "key"]);
    }
}

#[test]
fn foreign_providers_admit_iteration_frames_at_direct_and_encoded_scope_keys() {
    use mf_telemetry::event::ScopePathEntry;

    for encoded in [false, true] {
        let loop_key = if encoded {
            ScopePathEntry::binding_key("repeat", "body")
        } else {
            "repeat".to_owned()
        };
        let iteration_key = if encoded {
            ScopePathEntry::binding_key("each", "body")
        } else {
            "each".to_owned()
        };
        let description: WorkflowDescription = serde_json::from_value(serde_json::json!({
            "version":"2026-10-23", "workflow_id":graph().workflow_id,
            "execution":{"mode":"single"},
            "nodes":[{"id":"repeat", "kind":"foreign.repeater"}],
            "execution_order":["repeat"], "data_edges":[], "control_edges":[],
            "loop_bodies":[{
                "path":[loop_key], "source":"state-input", "state_outputs":{"value":"value"},
                "nodes":[{"id":"state-input", "kind":"foreign.state-input"},
                         {"id":"each", "kind":"foreign.iterator"}],
                "execution_order":["state-input", "each"], "data_edges":[], "control_edges":[]
            }, {
                "path":[loop_key, iteration_key], "source":"item-input", "state_outputs":{},
                "nodes":[{"id":"item-input", "kind":"foreign.item-input"},
                         {"id":"work", "kind":"fixture.work"}],
                "execution_order":["item-input", "work"], "data_edges":[], "control_edges":[]
            }]
        }))
        .unwrap();
        let mut item = record(1, 0, 1, IterationEvent::ItemStarted);
        item.context.iteration_path = vec![loop_key.clone(), iteration_key];
        item.context.body_path = item.context.iteration_path.clone();
        item.context.parent_path = vec![ScopePathEntry::Loop {
            loop_id: loop_key.clone(),
            index: Count::ZERO,
        }];
        let mut state = SessionState::new(description.clone(), RunId::new()).unwrap();
        assert_eq!(
            state.apply_iteration(item.clone()).unwrap(),
            Admission::Applied
        );
        let snapshot = state.snapshot();
        let nodes = snapshot.iterations.items[0].display_nodes(
            &item.context.iteration_path,
            [
                ("item-input", "foreign.item-input"),
                ("work", "fixture.work"),
            ],
        );
        assert_eq!(nodes[0].status, NodeStatus::Succeeded);

        // The same provider label cannot make a stateful frame into an Iteration.
        let mut stateful = description.clone();
        stateful.loop_bodies[1].state_outputs = [("value".into(), "value".into())].into();
        let mut state = SessionState::new(stateful, RunId::new()).unwrap();
        assert!(state.apply_iteration(item.clone()).is_err());

        let mut callable = description.clone();
        callable.loop_bodies[1].source = None;
        let mut state = SessionState::new(callable, RunId::new()).unwrap();
        assert!(state.apply_iteration(item.clone()).is_err());

        let mut wrong_parent_operator = item;
        wrong_parent_operator.context.parent_path =
            vec![ScopePathEntry::Call { call_id: loop_key }];
        let mut state = SessionState::new(description, RunId::new()).unwrap();
        assert!(state.apply_iteration(wrong_parent_operator).is_err());
    }
}

#[test]
fn parallel_items_and_repeated_calls_have_independent_live_nodes() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    for (invocation, index) in [(1, 0), (1, 1), (2, 0)] {
        let sequence = if invocation == 1 && index == 1 { 4 } else { 1 };
        state
            .apply_iteration(record(
                invocation,
                index,
                sequence,
                IterationEvent::ItemStarted,
            ))
            .unwrap();
        state
            .apply_iteration(record(invocation, index, sequence + 1, started()))
            .unwrap();
    }
    state
        .apply_iteration(record(1, 0, 3, finished(Outcome::Failed)))
        .unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.iterations.items.len(), 3);
    for item in &snapshot.iterations.items {
        let nodes = item.display_nodes(&["each".into()], [("work", "fixture.work")]);
        let expected = if item.identity.invocation == count(1) && item.identity.index == Count::ZERO
        {
            NodeStatus::Failed
        } else {
            NodeStatus::Running
        };
        assert_eq!(nodes[0].status, expected);
    }
    assert_eq!(
        snapshot
            .iterations
            .latest_node_event
            .unwrap()
            .context
            .invocation,
        count(2)
    );
    assert_eq!(
        snapshot.lifecycle,
        SessionState::new(graph(), RunId::new())
            .unwrap()
            .snapshot()
            .lifecycle
    );
}

#[test]
fn reordered_records_and_retransmissions_preserve_terminal_evidence() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    let terminal = record(1, 0, 3, finished(Outcome::Succeeded));
    state.apply_iteration(terminal.clone()).unwrap();
    let focus = state.snapshot().iterations.latest_node_event.clone();
    state
        .apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted))
        .unwrap();
    state.apply_iteration(record(1, 0, 2, started())).unwrap();
    assert_eq!(
        state.apply_iteration(terminal.clone()).unwrap(),
        Admission::Duplicate
    );
    assert_eq!(state.snapshot().iterations.latest_node_event, focus);
    let node = &state.snapshot().iterations.items[0].nodes[0].node;
    assert_eq!(node.status, NodeStatus::Succeeded);
    assert_eq!(node.started_elapsed_ns, Some(count(100_002)));
    let mut conflicting = terminal;
    conflicting.event = finished(Outcome::Failed);
    assert_eq!(
        state.apply_iteration(conflicting).unwrap(),
        Admission::Conflict
    );
    assert_eq!(
        state.snapshot().iterations.items[0].nodes[0].node.status,
        NodeStatus::Succeeded
    );
    assert_eq!(state.snapshot().iterations.protocol_conflicts, 1);
}

#[test]
fn missing_item_completion_is_unknown_after_invocation_closes() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    state
        .apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted))
        .unwrap();
    state.apply_iteration(record(1, 0, 2, started())).unwrap();
    state
        .apply_iteration(record(1, 0, 5, IterationEvent::InvocationFinished))
        .unwrap();
    let snapshot = state.snapshot();
    let item = &snapshot.iterations.items[0];
    assert!(item.closed);
    let node = &item.display_nodes(&["each".into()], [("work", "fixture.work")])[0];
    assert_eq!(node.status, NodeStatus::Unknown);
    assert!(node.possibly_missing_events);
    assert_eq!(snapshot.iterations.known_missing_count, 2);
    state
        .apply_iteration(record(1, 0, 3, finished(Outcome::Succeeded)))
        .unwrap();
    state
        .apply_iteration(record(
            1,
            0,
            4,
            IterationEvent::ItemFinished {
                outcome: Outcome::Succeeded,
                duration_ns: Some(count(10)),
            },
        ))
        .unwrap();
    let complete = state.snapshot();
    assert_eq!(complete.iterations.known_missing_count, 0);
    assert_eq!(
        complete.iterations.items[0].nodes[0].node.status,
        NodeStatus::Succeeded
    );
    assert!(
        state
            .apply_iteration(record(1, 0, 6, started()))
            .is_ok_and(|admission| admission == Admission::Conflict)
    );
}

#[test]
fn closure_interrupts_active_items_without_inventing_child_outcomes() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    state
        .apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted))
        .unwrap();
    state.apply_iteration(record(1, 0, 2, started())).unwrap();
    state.close();
    let snapshot = state.snapshot();
    let item = &snapshot.iterations.items[0];
    assert!(item.interrupted);
    let node = &item.display_nodes(&["each".into()], [("work", "fixture.work")])[0];
    assert_eq!(node.status, NodeStatus::Interrupted);
    assert_eq!(node.last_known, Some(NodeStatus::Running));
    assert!(
        state
            .apply_iteration(record(1, 0, 3, finished(Outcome::Succeeded)))
            .is_err()
    );
}

#[test]
fn completed_history_is_bounded_and_evicted_items_cannot_restart() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    for index in 0..100 {
        let sequence = 1 + index * 4;
        if index != 0 {
            state
                .apply_iteration(record(1, index, sequence, IterationEvent::ItemStarted))
                .unwrap();
        }
        state
            .apply_iteration(record(1, index, sequence + 1, started()))
            .unwrap();
        state
            .apply_iteration(record(1, index, sequence + 2, finished(Outcome::Succeeded)))
            .unwrap();
        state
            .apply_iteration(record(
                1,
                index,
                sequence + 3,
                IterationEvent::ItemFinished {
                    outcome: Outcome::Succeeded,
                    duration_ns: Some(count(1)),
                },
            ))
            .unwrap();
    }
    assert_eq!(state.snapshot().iterations.items.len(), 64);
    assert!(state.snapshot().iterations.retention_overflows >= 36);
    assert_eq!(
        state
            .apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted))
            .unwrap(),
        Admission::Duplicate
    );
    assert!(
        state
            .snapshot()
            .iterations
            .items
            .iter()
            .all(|item| item.identity.index != Count::ZERO)
    );
}

#[test]
fn invalid_scope_and_node_identity_never_update_the_graph() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    let mut invalid = record(1, 0, 1, started());
    invalid.context.body_path = vec!["missing".into()];
    assert!(state.apply_iteration(invalid).is_err());
    let mut invalid = record(1, 0, 1, started());
    if let IterationEvent::NodeStarted { node, .. } = &mut invalid.event {
        node.kind = "wrong.kind".into();
    }
    assert!(state.apply_iteration(invalid).is_err());
    assert!(state.snapshot().iterations.items.is_empty());
    assert_eq!(state.snapshot().lifecycle.known_missing_count, 0);
}

#[test]
fn delayed_detail_events_do_not_take_focus_from_newer_outer_nodes() {
    let description = graph();
    let run = RunId::new();
    let mut state = SessionState::new(description.clone(), run).unwrap();
    state
        .apply(LifecycleEvent {
            workflow_id: description.workflow_id,
            run_id: run,
            sequence: count(2),
            event: Event::NodeStarted {
                selected_inputs: Vec::new(),
                node: NodeIdentity {
                    id: "each".into(),
                    kind: "builtin.iteration".into(),
                    path: Vec::new(),
                },
                position: Count::ZERO,
                elapsed_ns: count(200_000),
            },
        })
        .unwrap();
    let revision = state.snapshot().latest_node_revision;
    state.apply_iteration(record(1, 0, 1, started())).unwrap();
    assert!(
        state
            .snapshot()
            .iterations
            .latest_node_event
            .unwrap()
            .revision
            < revision
    );
    let mut newer = record(1, 0, 2, finished(Outcome::Succeeded));
    newer.context.elapsed_ns = count(300_000);
    state.apply_iteration(newer).unwrap();
    assert!(
        state
            .snapshot()
            .iterations
            .latest_node_event
            .unwrap()
            .revision
            > revision
    );
}

#[test]
fn skipped_nodes_and_conflicting_transitions_preserve_the_observed_outcome() {
    use mf_telemetry::event::SkipCause;
    let mut description = graph();
    description.loop_bodies[0]
        .data_edges
        .push(mf_telemetry::description::DataEdge {
            exclusive: false,
            from_node: "%iteration".into(),
            from_output: "item".into(),
            to_node: "work".into(),
            to_input: "item".into(),
        });
    let mut state = SessionState::new(description, RunId::new()).unwrap();
    let mut skipped = record(1, 0, 2, started());
    skipped.event = IterationEvent::NodeSkipped {
        node: NodeIdentity {
            id: "work".into(),
            kind: "fixture.work".into(),
            path: Vec::new(),
        },
        causes: vec![SkipCause::Output {
            source_node: "%iteration".into(),
            source_output: "item".into(),
        }],
        skipped_ports: vec!["value".into()],
    };
    state.apply_iteration(skipped).unwrap();
    assert_eq!(
        state.apply_iteration(record(1, 0, 3, started())).unwrap(),
        Admission::Conflict
    );
    let snapshot = state.snapshot();
    let node = &snapshot.iterations.items[0].nodes[0].node;
    assert_eq!(node.status, NodeStatus::Skipped);
    assert!(node.conflicted);
    assert_eq!(node.skip_causes.len(), 1);
    assert_eq!(node.skipped_ports, ["value"]);
    assert_eq!(
        snapshot.iterations.items[0].display_nodes(&["each".into()], [("unseen", "fixture.work")])
            [0]
        .status,
        NodeStatus::Unknown
    );
    state
        .apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted))
        .unwrap();
    assert_eq!(
        state
            .apply_iteration(record(1, 0, 4, IterationEvent::ItemStarted))
            .unwrap(),
        Admission::Conflict
    );
    state
        .apply_iteration(record(
            1,
            0,
            5,
            IterationEvent::ItemFinished {
                outcome: Outcome::Succeeded,
                duration_ns: None,
            },
        ))
        .unwrap();
    assert_eq!(
        state
            .apply_iteration(record(1, 0, 6, finished(Outcome::Failed)))
            .unwrap(),
        Admission::Conflict
    );
    assert_eq!(
        state.snapshot().iterations.items[0]
            .display_nodes(&["each".into()], [("unseen", "fixture.work")])[0]
            .status,
        NodeStatus::Unknown
    );
}

#[test]
fn invocation_limits_retire_completed_history_and_protect_active_work() {
    for finish in [false, true] {
        let mut state = SessionState::new(graph(), RunId::new()).unwrap();
        for invocation in 1..=64 {
            state
                .apply_iteration(record(invocation, 0, 1, IterationEvent::ItemStarted))
                .unwrap();
            if finish {
                state
                    .apply_iteration(record(invocation, 0, 2, IterationEvent::InvocationFinished))
                    .unwrap();
            }
        }
        let admission = state
            .apply_iteration(record(65, 0, 1, IterationEvent::ItemStarted))
            .unwrap();
        assert_eq!(
            admission,
            if finish {
                Admission::Applied
            } else {
                Admission::Conflict
            }
        );
        assert_eq!(state.snapshot().iterations.items.len(), 64);
        assert!(state.snapshot().iterations.retention_overflows > 0);
        if finish {
            assert_eq!(
                state
                    .apply_iteration(record(1, 0, 1, IterationEvent::ItemStarted))
                    .unwrap(),
                Admission::Duplicate
            );
            let mut conflict = record(65, 0, 2, started());
            conflict.context.parent_stream_invocation = Some(9);
            assert_eq!(
                state.apply_iteration(conflict).unwrap(),
                Admission::Conflict
            );
        }
    }
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    for index in 0..64 {
        state
            .apply_iteration(record(1, index, index + 1, IterationEvent::ItemStarted))
            .unwrap();
    }
    assert_eq!(
        state
            .apply_iteration(record(1, 64, 65, IterationEvent::ItemStarted))
            .unwrap(),
        Admission::Conflict
    );
    assert_eq!(state.snapshot().iterations.items.len(), 64);
}

#[test]
fn invalid_detail_correlations_and_oversized_payloads_are_rejected_before_admission() {
    let mut state = SessionState::new(graph(), RunId::new()).unwrap();
    let mut invalid = record(1, 0, 1, started());
    invalid.correlation = Some((vec![1], vec![2]));
    assert!(state.apply_iteration(invalid).is_err());
    let mut invalid = record(1, 0, 1, started());
    invalid.context.iteration_path = vec!["missing".into()];
    invalid.context.body_path = invalid.context.iteration_path.clone();
    assert!(state.apply_iteration(invalid).is_err());
    let mut invalid = record(1, 0, 1, finished(Outcome::Failed));
    if let IterationEvent::NodeFinished {
        failure: Some(failure),
        ..
    } = &mut invalid.event
    {
        failure.message = "x".repeat(1024 * 1024);
    }
    assert!(state.apply_iteration(invalid).is_err());
    assert!(state.snapshot().iterations.items.is_empty());
    assert_eq!(state.snapshot().lifecycle.known_missing_count, 0);
}
