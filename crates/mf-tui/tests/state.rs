use mf_telemetry::{
    Count,
    description::{DataEdge, NodeDescription, WorkflowDescription, WorkflowDescriptionVersion},
    event::{Event, Failure, FailurePhase, LifecycleEvent, NodeIdentity, Outcome, SkipCause},
    identity::{RunId, WorkflowId},
};
use mf_tui::state::{Admission, Completeness, NodeStatus, SessionState};

fn count(value: i64) -> Count {
    Count::try_from(value).unwrap()
}

fn graph() -> WorkflowDescription {
    WorkflowDescription {
        version: WorkflowDescriptionVersion::CURRENT,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
        nodes: vec![
            NodeDescription {
                id: "a".into(),
                kind: "fixture.source".into(),
            },
            NodeDescription {
                id: "b".into(),
                kind: "fixture.sink".into(),
            },
        ],
        data_edges: vec![DataEdge {
            from_node: "a".into(),
            from_output: "value".into(),
            to_node: "b".into(),
            to_input: "input".into(),
        }],
        control_edges: vec![],
        execution_order: vec!["a".into(), "b".into()],
    }
}

fn run_id() -> RunId {
    RunId::try_from("12345678-1234-4234-9234-123456789abc".to_owned()).unwrap()
}

fn node(id: &str, kind: &str) -> NodeIdentity {
    NodeIdentity {
        id: id.into(),
        kind: kind.into(),
    }
}

fn event(sequence: i64, body: Event) -> LifecycleEvent {
    LifecycleEvent {
        workflow_id: graph().workflow_id,
        run_id: run_id(),
        sequence: count(sequence),
        event: body,
    }
}

fn start() -> LifecycleEvent {
    event(
        1,
        Event::WorkflowStarted {
            node_count: count(2),
            elapsed_ns: count(0),
        },
    )
}

fn node_start(sequence: i64, id: &str, kind: &str, position: i64) -> LifecycleEvent {
    event(
        sequence,
        Event::NodeStarted {
            node: node(id, kind),
            position: count(position),
            elapsed_ns: count(sequence * 10),
        },
    )
}

fn node_success(sequence: i64, id: &str, kind: &str, position: i64) -> LifecycleEvent {
    event(
        sequence,
        Event::NodeFinished {
            node: node(id, kind),
            position: count(position),
            elapsed_ns: count(sequence * 10),
            duration_ns: Some(count(5)),
            outcome: Outcome::Succeeded,
            produced_ports: vec!["value".into()],
            skipped_ports: vec![],
            failure: None,
        },
    )
}

fn success() -> LifecycleEvent {
    event(
        6,
        Event::WorkflowFinished {
            final_sequence: count(6),
            elapsed_ns: count(60),
            visited_node_count: count(2),
            outcome: Outcome::Succeeded,
            failure_node_id: None,
            failure: None,
        },
    )
}

fn state() -> SessionState {
    SessionState::new(graph(), run_id()).unwrap()
}

#[test]
fn finish_before_start_preserves_terminal_state_and_late_gaps_close() {
    let mut state = state();
    assert_eq!(
        state
            .apply(node_success(3, "a", "fixture.source", 0))
            .unwrap(),
        Admission::Applied
    );
    assert_eq!(state.snapshot().nodes[0].status, NodeStatus::Succeeded);
    assert_eq!(state.apply(success()).unwrap(), Admission::Applied);
    let pending = state.integrity();
    assert_eq!(pending.completeness, Completeness::Collecting);
    assert_eq!(pending.known_missing_count, 4);
    assert_eq!(pending.known_missing_ranges[0].first, 1);
    assert_eq!(state.apply(start()).unwrap(), Admission::Applied);
    assert_eq!(
        state
            .apply(node_start(2, "a", "fixture.source", 0))
            .unwrap(),
        Admission::Applied
    );
    let after_start = state.snapshot();
    assert_eq!(after_start.nodes[0].status, NodeStatus::Succeeded);
    assert_eq!(after_start.nodes[0].started_elapsed_ns, Some(count(20)));
    assert_eq!(
        state.apply(node_start(4, "b", "fixture.sink", 1)).unwrap(),
        Admission::Applied
    );
    assert_eq!(
        state
            .apply(node_success(5, "b", "fixture.sink", 1))
            .unwrap(),
        Admission::Applied
    );
    assert_eq!(state.integrity().completeness, Completeness::Complete);
    assert_eq!(state.integrity().known_missing_count, 0);
    assert_eq!(state.snapshot().workflow_outcome, Some(Outcome::Succeeded));
}

#[test]
fn duplicate_and_conflicting_events_never_regress_terminal_nodes() {
    let mut state = state();
    state.apply(start()).unwrap();
    let first = node_start(2, "a", "fixture.source", 0);
    assert_eq!(state.apply(first.clone()).unwrap(), Admission::Applied);
    assert_eq!(state.apply(first).unwrap(), Admission::Duplicate);
    assert_eq!(
        state.apply(node_start(2, "b", "fixture.sink", 1)).unwrap(),
        Admission::Conflict
    );
    let snapshot = state.snapshot();
    assert!(snapshot.nodes[0].conflicted && snapshot.nodes[1].conflicted);
    state
        .apply(node_success(3, "a", "fixture.source", 0))
        .unwrap();
    assert_eq!(
        state
            .apply(node_start(4, "a", "fixture.source", 0))
            .unwrap(),
        Admission::Conflict
    );
    assert_eq!(state.snapshot().nodes[0].status, NodeStatus::Succeeded);
    state.apply(success()).unwrap();
    state.close();
    assert_eq!(state.integrity().completeness, Completeness::Incomplete);
    assert!(state.integrity().protocol_conflicts >= 2);
}

#[test]
fn final_prefix_and_missing_tail_keep_unknowns_explicit() {
    let mut empty = state();
    empty.close();
    assert_eq!(empty.integrity().completeness, Completeness::UnverifiedTail);
    assert_eq!(empty.integrity().known_missing_count, 0);
    assert!(
        empty
            .snapshot()
            .nodes
            .iter()
            .all(|node| node.status == NodeStatus::Unknown)
    );

    let mut partial = state();
    partial.apply(start()).unwrap();
    partial
        .apply(node_start(2, "a", "fixture.source", 0))
        .unwrap();
    partial.close();
    assert_eq!(
        partial.integrity().completeness,
        Completeness::UnverifiedTail
    );
    assert_eq!(partial.snapshot().nodes[0].status, NodeStatus::Interrupted);
    assert_eq!(
        partial.snapshot().nodes[0].last_known,
        Some(NodeStatus::Running)
    );
    assert_eq!(partial.snapshot().nodes[1].status, NodeStatus::Unknown);

    let mut preparation = state();
    preparation
        .apply(event(
            2,
            Event::WorkflowFinished {
                final_sequence: count(2),
                elapsed_ns: count(3),
                visited_node_count: count(0),
                outcome: Outcome::Failed,
                failure_node_id: Some("b".into()),
                failure: Some(Failure {
                    phase: FailurePhase::Preparation,
                    message: "bad config".into(),
                }),
            },
        ))
        .unwrap();
    assert_eq!(preparation.snapshot().nodes[0].status, NodeStatus::NotRun);
    assert_eq!(preparation.snapshot().nodes[1].status, NodeStatus::Failed);
    preparation.close();
    assert_eq!(
        preparation.integrity().completeness,
        Completeness::Incomplete
    );
    assert_eq!(preparation.integrity().known_missing_count, 1);
}

#[test]
fn local_loss_trace_loss_and_diagnostic_truncation_stay_separate() {
    let mut state = state();
    state.apply(start()).unwrap();
    state.apply(success()).unwrap();
    state.record_local_lifecycle_drop(2, "local queue rejected records");
    state.record_trace_span();
    state.record_local_trace_drop(3, "trace queue rejected records");
    state.record_diagnostic_truncation(100);
    state.close();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Incomplete);
    assert_eq!(snapshot.lifecycle.known_missing_count, 4);
    assert_eq!(snapshot.lifecycle.local_drops, 2);
    assert_eq!(snapshot.traces.observed_spans, 1);
    assert_eq!(snapshot.traces.local_drops, 3);
    assert_eq!(snapshot.diagnostic_bytes_dropped, 100);
}

#[test]
fn interior_gap_and_trace_only_loss_have_distinct_integrity() {
    let mut missing = state();
    for event in [
        start(),
        node_start(2, "a", "fixture.source", 0),
        node_start(4, "b", "fixture.sink", 1),
        node_success(5, "b", "fixture.sink", 1),
        success(),
    ] {
        missing.apply(event).unwrap();
    }
    let active = missing.snapshot();
    assert_eq!(active.lifecycle.completeness, Completeness::Collecting);
    assert_eq!(active.lifecycle.known_missing_ranges[0].first, 3);
    assert_eq!(active.lifecycle.known_missing_ranges[0].last, 3);
    missing.close();
    assert_eq!(missing.integrity().completeness, Completeness::Incomplete);
    assert_eq!(missing.integrity().known_missing_count, 1);

    let mut trace_only = state();
    for event in [
        start(),
        node_start(2, "a", "fixture.source", 0),
        node_success(3, "a", "fixture.source", 0),
        node_start(4, "b", "fixture.sink", 1),
        node_success(5, "b", "fixture.sink", 1),
        success(),
    ] {
        trace_only.apply(event).unwrap();
    }
    trace_only.record_local_trace_drop(4, "trace exporter dropped spans");
    trace_only.record_diagnostic_truncation(25);
    trace_only.close();
    let snapshot = trace_only.snapshot();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
    assert_eq!(snapshot.traces.local_drops, 4);
    assert_eq!(snapshot.diagnostic_bytes_dropped, 25);
}

#[test]
fn a_known_gap_marks_only_unresolved_nodes_as_possibly_stale() {
    let mut state = state();
    state.apply(start()).unwrap();
    state
        .apply(node_success(3, "a", "fixture.source", 0))
        .unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.lifecycle.known_missing_count, 1);
    assert!(!snapshot.nodes[0].possibly_missing_events);
    assert_eq!(snapshot.nodes[0].status, NodeStatus::Succeeded);
    assert!(snapshot.nodes[1].possibly_missing_events);
    assert_eq!(snapshot.nodes[1].status, NodeStatus::Pending);
}

#[test]
fn visited_prefix_uses_execution_order_and_final_failure_identity() {
    let mut description = graph();
    description.nodes.reverse();
    let mut state = SessionState::new(description, run_id()).unwrap();
    state.apply(start()).unwrap();
    state
        .apply(node_start(2, "a", "fixture.source", 0))
        .unwrap();
    state
        .apply(event(
            3,
            Event::WorkflowFinished {
                final_sequence: count(3),
                elapsed_ns: count(30),
                visited_node_count: count(1),
                outcome: Outcome::Failed,
                failure_node_id: Some("a".into()),
                failure: Some(Failure {
                    phase: FailurePhase::Execution,
                    message: "execution failed".into(),
                }),
            },
        ))
        .unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.nodes[0].id, "a");
    assert_eq!(snapshot.nodes[0].status, NodeStatus::Failed);
    assert_eq!(snapshot.workflow_outcome, Some(Outcome::Failed));
    assert_eq!(
        snapshot.workflow_failure.as_ref().unwrap().message,
        "execution failed"
    );
    assert_eq!(snapshot.nodes[1].id, "b");
    assert_eq!(snapshot.nodes[1].status, NodeStatus::NotRun);
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
}

#[test]
fn late_start_can_fill_timing_for_a_failure_proven_by_final_boundary() {
    let mut state = state();
    state
        .apply(event(
            3,
            Event::WorkflowFinished {
                final_sequence: count(3),
                elapsed_ns: count(30),
                visited_node_count: count(1),
                outcome: Outcome::Failed,
                failure_node_id: Some("a".into()),
                failure: Some(Failure {
                    phase: FailurePhase::Execution,
                    message: "execution failed".into(),
                }),
            },
        ))
        .unwrap();
    state.apply(start()).unwrap();
    assert_eq!(
        state
            .apply(node_start(2, "a", "fixture.source", 0))
            .unwrap(),
        Admission::Applied
    );
    let snapshot = state.snapshot();
    assert_eq!(snapshot.nodes[0].status, NodeStatus::Failed);
    assert_eq!(snapshot.nodes[0].started_elapsed_ns, Some(count(20)));
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
    assert_eq!(snapshot.lifecycle.protocol_conflicts, 0);
}

#[test]
fn a_contiguous_stream_without_visited_node_outcomes_is_not_complete() {
    let mut state = state();
    state.apply(start()).unwrap();
    state
        .apply(event(
            2,
            Event::WorkflowFinished {
                final_sequence: count(2),
                elapsed_ns: count(20),
                visited_node_count: count(2),
                outcome: Outcome::Succeeded,
                failure_node_id: None,
                failure: None,
            },
        ))
        .unwrap();
    state.close();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.lifecycle.known_missing_count, 0);
    assert_eq!(snapshot.lifecycle.unresolved_visited_nodes, 2);
    assert_eq!(snapshot.workflow_outcome, Some(Outcome::Succeeded));
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Incomplete);
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.status == NodeStatus::Unknown)
    );
}

#[test]
fn unrelated_runs_and_out_of_range_sequences_do_not_change_state() {
    let mut state = state();
    let mut unrelated = start();
    unrelated.run_id = RunId::new();
    assert_eq!(state.apply(unrelated).unwrap(), Admission::UnrelatedRun);
    assert_eq!(state.integrity().known_missing_count, 0);
    let out_of_range = node_start(7, "a", "fixture.source", 0);
    assert!(state.apply(out_of_range).is_err());
    assert_eq!(state.integrity().known_missing_count, 0);
}

#[test]
fn missing_ranges_and_diagnostic_history_have_fixed_display_budgets() {
    let mut description = graph();
    description.nodes = (0..130)
        .map(|position| NodeDescription {
            id: format!("node_{position}"),
            kind: "fixture.empty".into(),
        })
        .collect();
    description.execution_order = description
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    description.data_edges.clear();
    let mut state = SessionState::new(description.clone(), run_id()).unwrap();
    for position in 0..70 {
        state
            .apply(LifecycleEvent {
                workflow_id: description.workflow_id.clone(),
                run_id: run_id(),
                sequence: count(position * 2 + 3),
                event: Event::NodeStarted {
                    node: node(&format!("node_{position}"), "fixture.empty"),
                    position: count(position),
                    elapsed_ns: count(position),
                },
            })
            .unwrap();
    }
    for index in 0..100 {
        state.record_diagnostic(&format!("diagnostic {index}"));
    }
    let snapshot = state.snapshot();
    assert_eq!(snapshot.lifecycle.known_missing_ranges.len(), 64);
    assert!(snapshot.lifecycle.omitted_missing_ranges > 0);
    assert_eq!(snapshot.diagnostics.len(), 64);
    assert_eq!(snapshot.diagnostics[0], "diagnostic 36");
    assert!(snapshot.diagnostic_bytes_dropped > 0);

    description
        .nodes
        .extend(
            (130..=mf_tui::state::MAX_SESSION_NODES).map(|position| NodeDescription {
                id: format!("node_{position}"),
                kind: "fixture.empty".into(),
            }),
        );
    description.execution_order = description
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect();
    assert!(SessionState::new(description, run_id()).is_err());
}

#[test]
fn conditional_skip_keeps_its_cause_and_inactive_port_outcome() {
    let mut state = state();
    state.apply(start()).unwrap();
    state
        .apply(node_start(2, "a", "fixture.source", 0))
        .unwrap();
    state
        .apply(event(
            3,
            Event::NodeFinished {
                node: node("a", "fixture.source"),
                position: count(0),
                elapsed_ns: count(30),
                duration_ns: Some(count(5)),
                outcome: Outcome::Succeeded,
                produced_ports: vec![],
                skipped_ports: vec!["value".into()],
                failure: None,
            },
        ))
        .unwrap();
    let skip = event(
        4,
        Event::NodeSkipped {
            node: node("b", "fixture.sink"),
            position: count(1),
            elapsed_ns: count(40),
            causes: vec![SkipCause {
                source_node: "a".into(),
                source_output: "value".into(),
            }],
            skipped_ports: vec!["result".into()],
        },
    );
    let mut wrong = skip.clone();
    if let Event::NodeSkipped { causes, .. } = &mut wrong.event {
        causes[0].source_output = "not-connected".into();
    }
    assert!(state.apply(wrong).is_err());
    state.apply(skip).unwrap();
    state
        .apply(event(
            5,
            Event::WorkflowFinished {
                final_sequence: count(5),
                elapsed_ns: count(50),
                visited_node_count: count(2),
                outcome: Outcome::Succeeded,
                failure_node_id: None,
                failure: None,
            },
        ))
        .unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.nodes[0].skipped_ports, ["value"]);
    assert_eq!(snapshot.nodes[1].status, NodeStatus::Skipped);
    assert_eq!(snapshot.nodes[1].skip_causes[0].source_node, "a");
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
}

#[test]
fn failure_and_diagnostic_text_have_independent_display_limits() {
    let mut state = state();
    state.apply(start()).unwrap();
    state
        .apply(node_start(2, "a", "fixture.source", 0))
        .unwrap();
    let failure = Failure {
        phase: FailurePhase::Execution,
        message: "x".repeat(5000),
    };
    state
        .apply(event(
            3,
            Event::NodeFinished {
                node: node("a", "fixture.source"),
                position: count(0),
                elapsed_ns: count(30),
                duration_ns: Some(count(5)),
                outcome: Outcome::Failed,
                produced_ports: vec![],
                skipped_ports: vec![],
                failure: Some(failure.clone()),
            },
        ))
        .unwrap();
    state
        .apply(event(
            4,
            Event::WorkflowFinished {
                final_sequence: count(4),
                elapsed_ns: count(40),
                visited_node_count: count(1),
                outcome: Outcome::Failed,
                failure_node_id: Some("a".into()),
                failure: Some(failure),
            },
        ))
        .unwrap();
    state.record_diagnostic(&"y".repeat(1500));
    let snapshot = state.snapshot();
    assert_eq!(snapshot.nodes[0].status, NodeStatus::Failed);
    assert_eq!(
        snapshot.nodes[0].failure.as_ref().unwrap().message.len(),
        4096
    );
    assert_eq!(
        snapshot.nodes[0].failure.as_ref().unwrap().dropped_bytes,
        904
    );
    assert_eq!(snapshot.diagnostic_bytes_dropped, 476);
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
}

#[test]
fn late_node_failure_cannot_confirm_a_successful_final_boundary() {
    let mut state = state();
    state.apply(success()).unwrap();
    state.apply(start()).unwrap();
    state
        .apply(node_start(2, "a", "fixture.source", 0))
        .unwrap();
    assert_eq!(
        state
            .apply(event(
                3,
                Event::NodeFinished {
                    node: node("a", "fixture.source"),
                    position: count(0),
                    elapsed_ns: count(30),
                    duration_ns: Some(count(5)),
                    outcome: Outcome::Failed,
                    produced_ports: vec![],
                    skipped_ports: vec![],
                    failure: Some(Failure {
                        phase: FailurePhase::Execution,
                        message: "failed".into(),
                    }),
                },
            ))
            .unwrap(),
        Admission::Conflict
    );
    state.apply(node_start(4, "b", "fixture.sink", 1)).unwrap();
    state
        .apply(node_success(5, "b", "fixture.sink", 1))
        .unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot.lifecycle.known_missing_count, 0);
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Collecting);
    assert!(snapshot.nodes[0].conflicted);
    state.close();
    assert_eq!(state.integrity().completeness, Completeness::Incomplete);
}
