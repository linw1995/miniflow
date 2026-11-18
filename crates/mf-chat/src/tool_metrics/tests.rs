use super::*;
use ToolExecutionPhase::*;
use mf_telemetry::{
    event::{FlowOrigin, LifecycleEvent, NodeIdentity},
    identity::WorkflowId,
    iteration::IterationContext,
    stream::{StreamIdentity, StreamRecord, StreamTrigger},
};

fn count(value: i64) -> Count {
    value.try_into().unwrap()
}

fn workflow() -> WorkflowId {
    WorkflowId::from_definition(&"fixture", &[]).unwrap()
}

fn call(id: &str) -> LoopPathEntry {
    LoopPathEntry::Call { call_id: id.into() }
}

fn item(index: i64) -> LoopPathEntry {
    LoopPathEntry::Loop {
        loop_id: "each".into(),
        index: count(index),
    }
}

fn child(scope: &[LoopPathEntry], entry: LoopPathEntry) -> Vec<LoopPathEntry> {
    scope.iter().cloned().chain([entry]).collect()
}

fn inputs(id: &str, name: &str) -> ValueRef {
    ValueRef::from(serde_json::json!({
        "call": {"id": id, "name": name, "arguments": {"payload": "not retained"}}
    }))
}

fn observe_call(
    metrics: &ToolExecutionMetrics,
    run: RunId,
    path: Vec<LoopPathEntry>,
    sequence: i64,
    elapsed: i64,
    outcome: Option<LoopPassOutcome>,
) {
    let event = match outcome {
        Some(outcome) => Event::CallFinished {
            path,
            elapsed_ns: count(elapsed),
            outcome,
        },
        None => Event::CallStarted {
            path,
            elapsed_ns: count(elapsed),
        },
    };
    metrics.observe(
        LocalRecord::Finite(&LifecycleEvent {
            workflow_id: workflow(),
            run_id: run,
            sequence: count(sequence),
            event,
        }),
        false,
    );
}

#[test]
fn call_evidence_waits_for_both_boundaries_and_the_binding() {
    let metrics = ToolExecutionMetrics::default();
    let run = RunId::new();
    let scope = [call("parent")];
    let path = child(&scope, call("selected"));
    for _ in 0..2 {
        observe_call(
            &metrics,
            run,
            path.clone(),
            4,
            12,
            Some(LoopPassOutcome::Completed),
        );
    }
    assert!(metrics.snapshot().is_empty());
    metrics.observe_snapshot(run, &scope, &inputs("first", "lookup"));
    assert_eq!(metrics.snapshot()["first"].phase, None);
    observe_call(&metrics, run, path, 2, 10, None);
    metrics.close(false);
    assert_eq!(metrics.snapshot()["first"].phase, Some(Succeeded));
    assert_eq!(metrics.snapshot()["first"].duration_ns, Some(2));
    assert!(!format!("{metrics:?}").contains("not retained"));
}

#[test]
fn foreign_runs_and_preview_only_candidates_do_not_establish_execution() {
    let metrics = ToolExecutionMetrics::default();
    let run = RunId::new();
    let scope = [call("parent")];
    metrics.observe_snapshot(run, &scope, &inputs("first", "lookup"));
    observe_call(
        &metrics,
        RunId::new(),
        child(&scope, call("selected")),
        2,
        10,
        None,
    );
    metrics.observe_snapshot(run, &[], &inputs("rejected", "unknown"));
    assert!(metrics.snapshot().is_empty());
}

#[test]
fn close_does_not_invent_success_and_accepts_no_later_evidence() {
    for interrupted in [false, true] {
        let metrics = ToolExecutionMetrics::default();
        let run = RunId::new();
        metrics.observe_snapshot(run, &[], &inputs("first", "lookup"));
        let path = vec![call("selected")];
        observe_call(&metrics, run, path.clone(), 2, 10, None);
        assert_eq!(metrics.snapshot()["first"].phase, Some(Running));
        metrics.close(interrupted);
        metrics.close(!interrupted);
        observe_call(&metrics, run, path, 4, 12, Some(LoopPassOutcome::Completed));
        assert_eq!(
            metrics.snapshot()["first"].phase,
            interrupted.then_some(Interrupted)
        );
    }
}

#[test]
fn repeated_wire_ids_and_conflicting_scope_claims_are_unknown() {
    let metrics = ToolExecutionMetrics::default();
    let run = RunId::new();
    let first = [call("first")];
    let second = [call("second")];
    metrics.observe_snapshot(run, &first, &inputs("id", "lookup"));
    metrics.observe_snapshot(run, &second, &inputs("id", "lookup"));
    metrics.observe_snapshot(run, &second, &inputs("other", "lookup"));
    metrics.observe_snapshot(run, &[call("third")], &inputs("name", "lookup"));
    metrics.observe_snapshot(run, &[call("third")], &inputs("name", "different"));
    observe_call(&metrics, run, child(&second, call("selected")), 2, 10, None);
    for id in ["id", "other", "name"] {
        assert_eq!(metrics.snapshot()[id].phase, None);
    }
}

#[test]
fn invalid_call_sequences_and_multiple_attempts_are_unknown() {
    let run = RunId::new();
    let scope = [call("parent")];
    for (end_sequence, end_time) in [(3, 9), (2, 11)] {
        let metrics = ToolExecutionMetrics::default();
        metrics.observe_snapshot(run, &scope, &inputs("first", "lookup"));
        observe_call(&metrics, run, child(&scope, call("one")), 2, 10, None);
        observe_call(
            &metrics,
            run,
            child(&scope, call("one")),
            end_sequence,
            end_time,
            Some(LoopPassOutcome::Completed),
        );
        assert_eq!(metrics.snapshot()["first"].phase, None);
    }
    for (second_invocation, second_key, rejected) in
        [(2, "a", false), (1, "b", false), (1, "a", true)]
    {
        let metrics = ToolExecutionMetrics::default();
        metrics.observe_snapshot(run, &scope, &inputs("first", "lookup"));
        for (invocation, key, conflict) in
            [(1, "a", false), (second_invocation, second_key, rejected)]
        {
            let record = StreamRecord {
                workflow_id: workflow(),
                run_id: run,
                sequence: count(2),
                identity: Some(StreamIdentity {
                    invocation,
                    message: None,
                    trigger: StreamTrigger::Startup,
                    parent: None,
                }),
                payload: StreamPayload::Execution(Event::CallStarted {
                    path: child(&scope, call(key)),
                    elapsed_ns: count(10),
                }),
                emission_count: None,
            };
            metrics.observe(LocalRecord::Stream(&record), conflict);
        }
        assert_eq!(metrics.snapshot()["first"].phase, None);
    }
}

struct IterationFixture {
    metrics: ToolExecutionMetrics,
    run: RunId,
    entry: LoopPathEntry,
}

impl IterationFixture {
    fn new(entry: LoopPathEntry) -> Self {
        Self {
            metrics: ToolExecutionMetrics::default(),
            run: RunId::new(),
            entry,
        }
    }

    fn bind(&self, index: i64, id: &str) {
        self.metrics
            .observe_snapshot(self.run, &[item(index)], &inputs(id, "lookup"));
    }

    fn record(
        &self,
        index: i64,
        body: bool,
        id: &str,
        seq: i64,
        outcome: Option<Outcome>,
    ) -> IterationRecord {
        let path = if body && self.entry.is_flow() {
            vec![self.entry.clone()]
        } else {
            vec![]
        };
        let node = NodeIdentity {
            id: id.into(),
            kind: "test".into(),
            path,
        };
        let mut body_path = vec!["each".into()];
        if body {
            body_path.push(self.entry.node_id().into());
        }
        IterationRecord {
            context: IterationContext {
                invocation: count(1),
                index: count(index),
                iteration_path: vec!["each".into()],
                body_path,
                parent_path: vec![],
                parent_stream_invocation: None,
                sequence: count(seq),
                elapsed_ns: count(seq * 10),
            },
            event: match outcome {
                None => IterationEvent::NodeStarted {
                    node,
                    selected_inputs: vec![],
                },
                Some(outcome) => IterationEvent::NodeFinished {
                    node,
                    outcome,
                    duration_ns: Some(count(10)),
                    produced_ports: vec![],
                    skipped_ports: vec![],
                    failure: None,
                },
            },
            correlation: None,
        }
    }

    fn send(&self, record: &IterationRecord) {
        self.metrics
            .observe(LocalRecord::Iteration(self.run, record), false);
    }
}

#[test]
fn iteration_tracks_the_exact_runtime_invoker_and_separates_parallel_items() {
    let fixture = IterationFixture::new(call(&LoopPathEntry::binding_key("execute", "named")));
    fixture.bind(0, "left");
    let right_started = fixture.record(1, true, "work", 4, None);
    fixture.send(&right_started);
    fixture.bind(1, "right");
    fixture.bind(2, "rejected");
    // A sibling can straddle the child but must not be treated as its owner.
    fixture.send(&fixture.record(0, false, "unrelated", 1, None));
    fixture.send(&fixture.record(0, false, "execute", 8, Some(Outcome::Succeeded)));
    fixture.send(&fixture.record(0, true, "work", 6, Some(Outcome::Succeeded)));
    assert_eq!(fixture.metrics.snapshot()["left"].phase, None);
    fixture.send(&fixture.record(0, true, "work", 3, None));
    assert_eq!(fixture.metrics.snapshot()["left"].phase, None);
    fixture.send(&fixture.record(0, false, "execute", 2, None));
    let view = fixture.metrics.snapshot();
    assert_eq!(view["left"].phase, Some(Succeeded));
    assert_eq!(view["left"].duration_ns, Some(50));
    assert_eq!(view["right"].phase, Some(Running));
    assert_eq!(view.len(), 2);
}

#[test]
fn iteration_preserves_business_failure_inside_a_successful_invoker() {
    let fixture = IterationFixture::new(LoopPathEntry::Flow {
        call_id: "execute".into(),
        definition: FlowOrigin {
            workflow_id: workflow(),
            path: vec!["producer".into()],
        },
    });
    fixture.bind(0, "failed");
    for (body, id, sequence, outcome) in [
        (false, "execute", 1, None),
        (true, "work", 2, None),
        (true, "work", 3, Some(Outcome::Failed)),
        (false, "execute", 4, Some(Outcome::Succeeded)),
    ] {
        fixture.send(&fixture.record(0, body, id, sequence, outcome));
    }
    fixture.metrics.close(true);
    assert_eq!(fixture.metrics.snapshot()["failed"].phase, Some(Failed));
    assert_eq!(fixture.metrics.snapshot()["failed"].duration_ns, Some(10));
}

#[test]
fn missing_or_conflicting_iteration_records_do_not_claim_a_terminal() {
    let fixture = IterationFixture::new(call("execute"));
    fixture.bind(0, "first");
    let started = fixture.record(0, true, "work", 3, None);
    fixture.send(&started);
    fixture.send(&fixture.record(0, false, "execute", 1, None));
    fixture.send(&fixture.record(0, false, "execute", 5, Some(Outcome::Succeeded)));
    assert_eq!(fixture.metrics.snapshot()["first"].phase, None);
    fixture.send(&fixture.record(0, true, "work", 4, Some(Outcome::Succeeded)));
    assert_eq!(fixture.metrics.snapshot()["first"].phase, Some(Succeeded));
    let mut skipped = started.clone();
    skipped.event = IterationEvent::NodeSkipped {
        node: started.event.node().unwrap().clone(),
        causes: vec![],
        skipped_ports: vec![],
    };
    fixture.send(&skipped);
    assert_eq!(fixture.metrics.snapshot()["first"].phase, None);
}

#[test]
fn conflicting_children_and_stale_iteration_attempts_are_unknown() {
    let mut fixture = IterationFixture::new(call("execute"));
    fixture.bind(0, "first");
    let started = fixture.record(0, true, "work", 2, None);
    fixture.send(&started);
    let mut stale = started;
    stale.context.invocation = count(2);
    fixture.send(&stale);
    assert_eq!(fixture.metrics.snapshot()["first"].phase, None);
    fixture.entry = call("other");
    fixture.send(&fixture.record(0, true, "work", 4, None));
    assert_eq!(fixture.metrics.snapshot()["first"].phase, None);
}

#[test]
fn malformed_labels_and_bounds_keep_the_projection_fail_closed() {
    let metrics = ToolExecutionMetrics::default();
    let run = RunId::new();
    for input in [
        ValueRef::null(),
        ValueRef::from(serde_json::json!({"call": {"id": "id"}})),
        ValueRef::from(serde_json::json!({"call": {"id": 1, "name": "test", "arguments": {}}})),
        inputs("[REDACTED]", "test"),
        inputs("id", "\n"),
        inputs("id", &"x".repeat(257)),
    ] {
        metrics.observe_snapshot(run, &[], &input);
    }
    assert!(metrics.0.lock().unwrap().scopes.is_empty());
    observe_call(&metrics, run, vec![call("selected")], 2, 10, None);
    for index in 0..=MAX_BINDINGS {
        let scope = if index == 0 {
            vec![]
        } else {
            vec![call(&index.to_string())]
        };
        metrics.observe_snapshot(run, &scope, &inputs(&index.to_string(), "lookup"));
    }
    let view = metrics.snapshot();
    assert_eq!(view.len(), MAX_BINDINGS);
    assert!(view.values().all(|tool| tool.phase.is_none()));

    let metrics = ToolExecutionMetrics::default();
    for index in 0..=MAX_SCOPES {
        observe_call(
            &metrics,
            run,
            vec![call(&index.to_string()), call("selected")],
            2,
            10,
            None,
        );
    }
    assert_eq!(metrics.0.lock().unwrap().scopes.len(), MAX_SCOPES);
    assert!(metrics.0.lock().unwrap().saturated);

    let fixture = IterationFixture::new(call("execute"));
    for index in 0..=MAX_NODES {
        fixture.send(&fixture.record(0, true, &index.to_string(), 4, None));
    }
    assert_eq!(fixture.metrics.0.lock().unwrap().node_count, MAX_NODES);
    assert!(fixture.metrics.0.lock().unwrap().saturated);
}
