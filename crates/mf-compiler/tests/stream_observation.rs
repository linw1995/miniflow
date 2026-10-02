#[path = "fixtures/observation_capture.rs"]
mod capture;
extern crate mfn_core as _;
use capture::Harness;
use mf_compiler::{NodeRegistry, WorkflowDefinition, plan_definition, start_stream};
use mf_runtime::{StreamClock, StreamOptions};
use mf_telemetry::{
    INSTRUMENTATION_SCOPE,
    event::{Event, Outcome},
    identity::RunId,
    stream::{
        StreamCounts, StreamEvent, StreamMessage, StreamOutcome, StreamPayload, StreamRecord,
        StreamTrigger,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    task::Waker,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Clock {
    time: Mutex<Duration>,
    wakers: Mutex<Vec<Waker>>,
}
impl StreamClock for Clock {
    fn now(&self) -> Duration {
        *self.time.lock().unwrap()
    }
    fn register_waker(&self, waker: Waker) {
        self.wakers.lock().unwrap().push(waker);
    }
}
impl Clock {
    fn advance(&self, value: u64) {
        *self.time.lock().unwrap() = Duration::from_millis(value);
        for waker in self.wakers.lock().unwrap().iter() {
            waker.wake_by_ref();
        }
    }
}
fn wait_until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
}
fn definition() -> Value {
    serde_json::from_str(include_str!("../../../examples/stream-batch.json")).unwrap()
}
fn plan(value: Value) -> mf_compiler::CompiledWorkflow {
    plan_definition(&serde_json::from_value::<WorkflowDefinition>(value).unwrap()).unwrap()
}
fn records(harness: &Harness) -> Vec<StreamRecord> {
    harness
        .records()
        .iter()
        .filter(|record| record.scope == INSTRUMENTATION_SCOPE)
        .map(|record| StreamRecord::decode(record).unwrap())
        .collect()
}

#[test]
fn buffering_flushes_and_repeated_messages_have_distinct_correlated_lifecycles() {
    for sampled in [false, true] {
        let harness = Harness::new(sampled);
        let mut value = definition();
        value["execution"]["input_type"] = json!("string");
        let plan = plan(value);
        let observation = plan
            .start_stream_observation(&harness.observer(), RunId::new())
            .unwrap();
        let instance = start_stream(
            &plan,
            &NodeRegistry::from_inventory().unwrap(),
            StreamOptions {
                observation: Some(observation),
                clock: Arc::new(Clock::default()),
                ..Default::default()
            },
        )
        .unwrap();
        for value in ["secret-a", "secret-b", "secret-c", "secret-d", "secret-e"] {
            instance.input().send(json!(value)).unwrap();
        }
        instance.close_input();
        assert!(instance.recv().unwrap().is_some());
        assert!(instance.recv().unwrap().is_some());
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
        let events = records(&harness);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.sequence.get(), index as i64 + 1);
        }
        let mut invocations = BTreeSet::new();
        let mut messages = BTreeSet::new();
        let mut zero_emissions = 0;
        let mut reasons = Vec::new();
        for event in &events {
            match &event.payload {
                StreamPayload::Execution(Event::NodeStarted { node, .. }) => {
                    let identity = event.identity.as_ref().unwrap();
                    assert!(invocations.insert(identity.invocation));
                    if node.id == "consume" {
                        let message = identity.message.unwrap();
                        messages.insert((message.domain, message.sequence));
                    }
                }
                StreamPayload::Execution(Event::NodeFinished {
                    outcome: Outcome::Succeeded,
                    ..
                }) if event.emission_count.is_some_and(|count| count.get() == 0) => {
                    zero_emissions += 1
                }
                StreamPayload::Control(StreamEvent::Flushed { reason, .. }) => {
                    reasons.push(reason.clone())
                }
                _ => {}
            }
        }
        assert!(zero_emissions >= 4);
        assert_eq!(messages, BTreeSet::from([(1, 0), (1, 1)]));
        assert_eq!(reasons, ["size_exceed", "upstream_closed"]);
        assert!(
            matches!(&events.last().unwrap().payload, StreamPayload::Control(StreamEvent::Finished { outcome: StreamOutcome::Succeeded, counts, .. }) if counts.accepted_inputs == 5 && counts.delivered_outputs == 2)
        );
        assert!(
            !serde_json::to_string(&harness.records())
                .unwrap()
                .contains("secret-")
        );
        if sampled {
            let spans = harness.spans.get_finished_spans().unwrap();
            let root = spans
                .iter()
                .find(|span| span.name == "mf.workflow")
                .unwrap();
            assert!(
                spans
                    .iter()
                    .filter(|span| span.name == "mf.node")
                    .all(|span| span.parent_span_id == root.span_context.span_id())
            );
        } else {
            assert!(harness.spans.get_finished_spans().unwrap().is_empty());
        }
    }
}

#[test]
fn timers_have_no_input_message_and_terminal_events_wait_for_delivery() {
    let harness = Harness::new(true);
    let plan = plan(definition());
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let clock = Arc::new(Clock::default());
    let instance = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            clock: clock.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    instance.input().send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 1);
    clock.advance(250);
    let delivery = instance.receive().unwrap().unwrap();
    instance.close_input();
    assert!(!records(&harness).iter().any(|record| matches!(
        record.payload,
        StreamPayload::Control(StreamEvent::Finished { .. })
    )));
    let events = records(&harness);
    let timeout = events.iter().find(|record| matches!(&record.payload, StreamPayload::Control(StreamEvent::Flushed { reason, .. }) if reason == "timeout_exceed")).unwrap();
    assert_eq!(
        timeout.identity.as_ref().unwrap().trigger,
        StreamTrigger::Timer
    );
    assert!(timeout.identity.as_ref().unwrap().message.is_none());
    delivery.acknowledge().unwrap();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    assert!(matches!(
        records(&harness).last().unwrap().payload,
        StreamPayload::Control(StreamEvent::Finished { .. })
    ));
}

#[test]
fn resource_failures_never_publish_success_and_snapshot_requests_fail_before_input() {
    let harness = Harness::new(true);
    let mut value = definition();
    value["nodes"][0]["config"]["max_items"] = json!(1);
    value["nodes"][1] =
        json!({"id":"consume", "kind":"builtin.constant", "config":{"value":"x".repeat(100)}});
    value["edges"].as_array_mut().unwrap().pop();
    value["control_edges"] =
        json!([{"from_node":"collect", "from_output":"items", "to_node":"consume"}]);
    value["execution"]["limits"] = json!({"max_message_bytes":32, "max_buffered_bytes":8192});
    let plan = plan(value);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let instance = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            ..Default::default()
        },
    )
    .unwrap();
    instance.input().send(json!(1)).unwrap();
    instance.close_input();
    assert!(instance.recv().is_err());
    assert!(instance.join().is_err());
    let events = records(&harness);
    assert!(events.iter().any(|event| matches!(&event.payload, StreamPayload::Execution(Event::NodeFinished { node, outcome: Outcome::Failed, .. }) if node.id == "consume")));
    assert!(!events.iter().any(|event| matches!(&event.payload, StreamPayload::Execution(Event::NodeFinished { node, outcome: Outcome::Succeeded, .. }) if node.id == "consume")));

    let harness = Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let result = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            snapshots: Some(mf_runtime::SnapshotRecorder::memory()),
            ..Default::default()
        },
    );
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("snapshot capture")
    );
    assert_eq!(records(&harness).len(), 2);
}

#[test]
fn loop_paths_and_iteration_details_keep_the_containing_message_identity() {
    let value = json!({"version":"2026-10-02", "execution":{"mode":"stream", "input_type":"int"}, "dependencies":{},
        "nodes":[
            {"id":"repeat", "kind":"workflow.loop", "loop":{"max_iterations":2, "variables":[{"name":"x", "type":"int"}], "body":{
                "nodes":[{"id":"copy", "kind":"builtin.identity"}], "edges":[{"from_node":"%loop", "from_output":"x", "to_node":"copy", "to_input":"input"}]}}},
            {"id":"collect", "kind":"builtin.batch", "config":{"max_items":1, "max_wait_ms":100}},
            {"id":"iterate", "kind":"builtin.iteration", "config":{"body":{"nodes":[{"id":"copy", "kind":"builtin.identity"}],
                "edges":[{"from_node":"%iteration", "from_output":"items", "to_node":"copy", "to_input":"input"}], "result":{"node":"copy", "port":"value"}}}}
        ], "edges":[{"from_node":"%input", "from_output":"item", "to_node":"repeat", "to_input":"x"},
            {"from_node":"repeat", "from_output":"x", "to_node":"collect", "to_input":"item"},
            {"from_node":"collect", "from_output":"items", "to_node":"iterate", "to_input":"items"}],
        "outputs":[{"name":"value", "node":"iterate", "port":"results"}]
    });
    let plan = plan(value);
    let harness = Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let instance = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            ..Default::default()
        },
    )
    .unwrap();
    instance.input().send(json!(1)).unwrap();
    instance.input().send(json!(2)).unwrap();
    instance.close_input();
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["value"],
        json!([1])
    );
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["value"],
        json!([2])
    );
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    let events = records(&harness);
    let loops: BTreeSet<_> = events
        .iter()
        .filter_map(|record| match &record.payload {
            StreamPayload::Execution(Event::NodeStarted { node, .. }) if !node.path.is_empty() => {
                Some(record.identity.as_ref().unwrap().message.unwrap().sequence)
            }
            _ => None,
        })
        .collect();
    assert_eq!(loops, BTreeSet::from([0, 1]));
    let details: Vec<_> = harness
        .records()
        .into_iter()
        .filter(|record| record.event_name == "mf.iteration.node.started")
        .collect();
    assert_eq!(details.len(), 2);
    assert_ne!(
        details[0].attributes["mf.stream.invocation"],
        details[1].attributes["mf.stream.invocation"]
    );
    assert_ne!(
        details[0].attributes["mf.stream.message"],
        details[1].attributes["mf.stream.message"]
    );
    assert!(
        details
            .iter()
            .all(|record| !record.attributes.contains_key("mf.event.sequence"))
    );
}

#[test]
fn stream_sequences_outlive_the_legacy_run_budget_and_close_exactly_once() {
    let harness = Harness::new(false);
    let plan = plan(definition());
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    for sequence in 0..20_010 {
        let mut call = observation
            .callback(
                "collect",
                Some(StreamMessage {
                    domain: 0,
                    sequence,
                }),
                StreamTrigger::Input,
            )
            .unwrap();
        call.started();
        call.succeeded(0, Vec::new());
    }
    observation.finish(StreamCounts::default(), None);
    observation.finish(StreamCounts::default(), None);
    assert!(
        observation
            .callback("collect", None, StreamTrigger::Timer)
            .is_none()
    );
    let events = records(&harness);
    assert!(events.last().unwrap().sequence > mf_telemetry::maximum_loop_event_count());
    assert_eq!(events.len(), 40_022);
    let wire = harness.records().pop().unwrap();
    assert_eq!(
        StreamRecord::decode(&wire).unwrap(),
        *events.last().unwrap()
    );
    assert!(wire.decode().is_err());
}

#[test]
fn a_nested_workflow_does_not_reuse_another_runs_invocation_identity() {
    let harness = Harness::new(true);
    let plan = plan(definition());
    let outer_id = RunId::new();
    let inner_id = RunId::new();
    let outer = plan
        .start_stream_observation(&harness.observer(), outer_id)
        .unwrap();
    let mut callback = outer
        .callback(
            "collect",
            Some(StreamMessage {
                domain: 0,
                sequence: 0,
            }),
            StreamTrigger::Input,
        )
        .unwrap();
    callback.started();
    {
        let _context = callback.enter();
        let inner = plan
            .start_stream_observation(&harness.observer(), inner_id)
            .unwrap();
        let mut call = inner
            .callback(
                "collect",
                Some(StreamMessage {
                    domain: 0,
                    sequence: 0,
                }),
                StreamTrigger::Input,
            )
            .unwrap();
        call.started();
        call.succeeded(0, Vec::new());
        inner.finish(StreamCounts::default(), None);
    }
    callback.succeeded(0, Vec::new());
    outer.finish(StreamCounts::default(), None);
    let events = records(&harness);
    let inner = events
        .iter()
        .find(|event| {
            event.run_id == inner_id
                && matches!(
                    event.payload,
                    StreamPayload::Execution(Event::NodeStarted { .. })
                )
        })
        .unwrap();
    assert!(inner.identity.as_ref().unwrap().parent.is_none());
}
