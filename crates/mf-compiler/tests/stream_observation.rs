#[path = "common/snapshots.rs"]
mod snapshots;
use snapshots::Snapshots;
#[path = "fixtures/controlled_source.rs"]
mod controlled;
use controlled::SourceRun;
#[path = "fixtures/observation_capture.rs"]
mod capture;
extern crate mfn_core as _;
use capture::Harness;
use mf_compiler::{NodeRegistry, WorkflowDefinition, plan_definition};
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

#[test]
fn snapshots_are_live_and_distinguish_buffering_from_timer_outputs() {
    use mf_runtime::SnapshotOutcome;
    let harness = Harness::new(true);
    let plan = plan(definition());
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let recording = Snapshots::memory();
    let clock = Arc::new(Clock::default());
    let instance = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            snapshots: Some(recording.recorder()),
            clock: clock.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    instance.source.send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    let current = recording.current();
    assert_eq!(current.node(&[], "feed").unwrap().outputs["item"], json!(1));
    assert_eq!(
        current.node(&[], "collect").unwrap().inputs["item"],
        json!(1)
    );
    assert!(
        current
            .node(&[], "collect")
            .unwrap()
            .outputs
            .as_object()
            .unwrap()
            .is_empty()
    );
    instance.source.send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 3);
    assert_eq!(
        recording
            .history()
            .iter()
            .filter(|entry| entry.node.as_ref() == "collect"
                && entry
                    .snapshot
                    .node(&entry.scope, &entry.node)
                    .unwrap()
                    .outcome
                    == SnapshotOutcome::Started)
            .count(),
        2
    );
    clock.advance(250);
    let delivery = instance.receive().unwrap().unwrap();
    assert_eq!(delivery.output().outputs["batch"], json!([1, 1]));
    delivery.acknowledge().unwrap();
    instance.source.close();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    recording.finish();
    assert!(recording.diagnostic().is_none());
    let emitted = recording
        .history()
        .into_iter()
        .filter(|entry| {
            entry.node.as_ref() == "feed"
                && entry
                    .snapshot
                    .node(&entry.scope, &entry.node)
                    .unwrap()
                    .outcome
                    == SnapshotOutcome::Emitted
        })
        .collect::<Vec<_>>();
    assert_eq!(emitted.len(), 2);
    assert!(
        emitted[0]
            .snapshot
            .node(&[], "feed")
            .unwrap()
            .outputs
            .eq(&emitted[1].snapshot.node(&[], "feed").unwrap().outputs)
    );
    assert_eq!(
        recording.current().node(&[], "collect").unwrap().outputs["items"],
        json!([1, 1])
    );
}

#[test]
fn secret_emissions_survive_batching_without_entering_snapshot_definitions() {
    let plan = plan(definition());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let records = captured.clone();
    let recording = Snapshots::with_sink(move |record| {
        records.lock().unwrap().push(record.clone());
        Ok(())
    })
    .unwrap();
    let instance = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            snapshots: Some(recording.recorder()),
            clock: Arc::new(Clock::default()),
            ..Default::default()
        },
    )
    .unwrap();
    let secret = mf_runtime::ValueRef::from(987654321).into_secret();
    instance.source.send(secret.clone()).unwrap();
    instance.source.send(7).unwrap();
    instance.source.close();
    let output = instance.recv().unwrap().unwrap();
    assert!(output.outputs["batch"][0].ptr_eq(&secret));
    assert_eq!(
        serde_json::to_value(&output.outputs["batch"]).unwrap(),
        json!([987654321, 7])
    );
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    recording.finish();
    assert!(recording.diagnostic().is_none());
    let records = captured.lock().unwrap();
    assert!(
        !serde_json::to_string(&*records)
            .unwrap()
            .contains("987654321")
    );
    assert_eq!(records.last().unwrap()["record"], "end");
    let collected = records
        .iter()
        .rev()
        .find(|record| record["record"] == "node" && record["node"] == "collect")
        .unwrap();
    assert_eq!(collected["outputs"]["items"], json!(["[REDACTED]", 7]));
}

struct PartialCapture;

impl mf_runtime::EventNode for PartialCapture {
    fn on_event(
        &mut self,
        event: mf_runtime::NodeEvent,
        _: &mf_runtime::EventContext<'_>,
    ) -> Result<mf_runtime::EventEffects, mf_runtime::NodeExecutionError> {
        if !matches!(event, mf_runtime::NodeEvent::Input(_)) {
            return Ok(Default::default());
        }
        Ok(mf_runtime::EventEffects {
            emissions: vec![
                mf_runtime::EventEmission {
                    result: mf_runtime::Outputs::from([("value".into(), json!(1).into())]).into(),
                    batch: None,
                },
                mf_runtime::EventEmission {
                    result: mf_runtime::Outputs::from([("value".into(), json!("invalid").into())])
                        .into(),
                    batch: None,
                },
            ],
            ..Default::default()
        })
    }
}
inventory::submit! { mf_runtime::NodeRegistration {
    kind: "test.partial_capture",
    factory: mf_runtime::NodeFactory::Plain(|_| Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Event(Box::new(PartialCapture)),
        mf_runtime::NodePorts {
            inputs: vec![mf_runtime::PortSpec::new("value", mf_runtime::ValueType::Int64, true)],
            outputs: vec![mf_runtime::PortSpec::new("value", mf_runtime::ValueType::Int64, true)],
        },
    ))),
} }

#[test]
fn partial_event_publication_keeps_only_admitted_data_and_failed_outcome() {
    let plan = plan(
        json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[{"id":"start", "kind":"builtin.identity"}, {"id":"partial","kind":"test.partial_capture"}],
        "edges":[{"from_node":"start","from_output":"value","to_node":"partial","to_input":"value"}],
        "outputs":[{"name":"value","node":"partial","port":"value"}]}),
    );
    let harness = Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let capture = Snapshots::memory();
    let instance = mf_compiler::start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            snapshots: Some(capture.recorder()),
            arguments: mf_runtime::WorkflowArguments::try_from(json!({"start.input":7})).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(instance.recv().is_err());
    assert!(instance.join().is_err());
    capture.finish();
    assert!(capture.diagnostic().is_none());
    let outputs = capture
        .history()
        .into_iter()
        .filter(|entry| {
            entry.node.as_ref() == "partial"
                && entry
                    .snapshot
                    .node(&entry.scope, &entry.node)
                    .unwrap()
                    .outcome
                    == mf_runtime::SnapshotOutcome::Emitted
        })
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].snapshot.node(&[], "partial").unwrap().outputs["value"],
        json!(1)
    );
    assert!(records(&harness).iter().any(|record| matches!(&record.payload, StreamPayload::Execution(Event::NodeFinished { node, outcome: Outcome::Failed, .. }) if node.id == "partial")));
}

#[test]
fn snapshot_callbacks_can_observe_runtime_metrics_without_locking_execution() {
    let harness = Harness::new(true);
    let plan = plan(definition());
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let holder = Arc::new(Mutex::new(
        None::<std::sync::Weak<mf_runtime::StreamInstance>>,
    ));
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured_holder = holder.clone();
    let captured_observed = observed.clone();
    let capture = mf_runtime::SnapshotRecorder::new(move |event| {
        if matches!(
            event.snapshot.outcome,
            mf_runtime::SnapshotOutcome::Emitted | mf_runtime::SnapshotOutcome::Succeeded
        ) && let Some(instance) = captured_holder
            .lock()
            .unwrap()
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
        {
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                let _ = sender.send(instance.metrics());
            });
            receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("runtime metrics must remain callable during data publication");
            worker.join().unwrap();
            captured_observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });
    let mut options = StreamOptions {
        observation: Some(observation),
        snapshots: Some(capture),
        ..Default::default()
    };
    let source = controlled::prepare_control("feed", &mut options);
    let instance = Arc::new(
        mf_compiler::start_stream(&plan, &NodeRegistry::from_inventory().unwrap(), options)
            .unwrap(),
    );
    *holder.lock().unwrap() = Some(Arc::downgrade(&instance));
    source.send(json!(1)).unwrap();
    source.close();
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([1])
    );
    assert!(instance.recv().unwrap().is_none());
    Arc::try_unwrap(instance).ok().unwrap().join().unwrap();
    assert!(observed.load(std::sync::atomic::Ordering::SeqCst) >= 3);
}

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
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/stream-batch.json")).unwrap();
    value.as_object_mut().unwrap().remove("clusters");
    value["dependencies"]
        .as_object_mut()
        .unwrap()
        .remove("code");
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] != "convert");
    value["nodes"][0]["inputs"]["item"] = json!({"from":{"node":"feed","port":"item"}});
    *value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "feed")
        .unwrap() = controlled::source(json!("int"));
    value
}
fn plan(value: Value) -> mf_compiler::CompiledWorkflow {
    plan_definition(&serde_json::from_value::<WorkflowDefinition>(value).unwrap()).unwrap()
}
fn start_stream(
    plan: &mf_compiler::CompiledWorkflow,
    registry: &NodeRegistry,
    mut options: StreamOptions,
) -> Result<SourceRun, mf_compiler::WorkflowExecutionError> {
    let source = controlled::prepare_control("feed", &mut options);
    let instance = mf_compiler::start_stream(plan, registry, options)?;
    Ok(SourceRun { instance, source })
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
fn startup_loop_observation_needs_no_synthetic_message_or_source() {
    let plan = plan(
        json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[{"id":"repeat", "kind":"workflow.loop", "loop":{"max_iterations":2,
            "variables":[{"name":"x", "type":"int"}], "body":{
                "nodes":[{"id":"copy", "kind":"builtin.identity"}],
                "edges":[{"from_node":"%loop", "from_output":"x", "to_node":"copy", "to_input":"input"}]
            }}}], "outputs":[{"name":"value", "node":"repeat", "port":"x"}]}),
    );
    let harness = Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let instance = mf_compiler::start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            arguments: mf_runtime::WorkflowArguments::try_from(json!({"repeat.x":7})).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(instance.recv().unwrap().unwrap().outputs["value"], json!(7));
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    let events = records(&harness);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence.get(), index as i64 + 1);
        assert_eq!(event.schema_version, 4);
        if let Some(identity) = &event.identity {
            assert_eq!(identity.trigger, StreamTrigger::Startup);
            assert!(identity.message.is_none());
        }
    }
    assert!(events.iter().any(|record| matches!(&record.payload, StreamPayload::Execution(Event::NodeStarted { node, .. }) if !node.path.is_empty())));
    assert!(
        matches!(&events.last().unwrap().payload, StreamPayload::Control(StreamEvent::Finished { counts, outcome: StreamOutcome::Succeeded, .. }) if counts.startup_frames == 1 && counts.emitted_messages == 0 && counts.completed_frames == 1)
    );
}

#[test]
fn invalid_startup_arguments_fail_before_any_node_started_record() {
    let plan = plan(
        json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[{"id":"copy", "kind":"builtin.identity"}]}),
    );
    let harness = Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    assert!(
        mf_compiler::start_stream(
            &plan,
            &NodeRegistry::from_inventory().unwrap(),
            StreamOptions {
                observation: Some(observation),
                ..Default::default()
            }
        )
        .is_err()
    );
    let events = records(&harness);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[1].payload, StreamPayload::Control(StreamEvent::Finished { counts, failure: Some(failure), .. }) if counts.startup_frames == 0 && failure.phase == "input")
    );
}

#[test]
fn buffering_flushes_and_repeated_messages_have_distinct_correlated_lifecycles() {
    for sampled in [false, true] {
        let harness = Harness::new(sampled);
        let mut value = definition();
        value["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["id"] == "feed")
            .unwrap()["config"]["item_type"] = json!("string");
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
            instance.source.clone().send(json!(value)).unwrap();
        }
        instance.source.close();
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
        assert_eq!(messages, BTreeSet::from([(2, 0), (2, 1)]));
        assert_eq!(reasons, ["size_exceed", "upstream_closed"]);
        assert!(
            matches!(&events.last().unwrap().payload, StreamPayload::Control(StreamEvent::Finished { outcome: StreamOutcome::Succeeded, counts, .. }) if counts.startup_frames == 1 && counts.emitted_messages == 7 && counts.delivered_outputs == 2)
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
    instance.source.clone().send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    clock.advance(250);
    let delivery = instance.receive().unwrap().unwrap();
    instance.source.close();
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
fn task_failures_preserve_outcomes_and_raw_snapshots_need_no_telemetry() {
    let harness = Harness::new(true);
    let mut value = definition();
    value["nodes"][0]["config"]["max_items"] = json!(1);
    value["nodes"][1] = json!({
        "id":"consume", "kind":"builtin.if_else",
        "config":{"branches":[{"id":"hit", "condition":{
            "source":{"output":"before.value", "path":""}, "operator":"gt", "value":0
        }}]}
    });
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"before", "kind":"builtin.identity",
            "inputs":{"input":{"from":{"node":"collect","port":"items"}}}}));
    value["outputs"][0]["port"] = json!("hit");
    value["edges"] = json!([{"from_node":"before", "from_output":"value", "to_node":"consume"}]);
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
    instance.source.clone().send(json!(1)).unwrap();
    instance.source.close();
    assert!(instance.recv().is_err());
    assert!(instance.join().is_err());
    let events = records(&harness);
    assert!(events.iter().any(|event| matches!(&event.payload, StreamPayload::Execution(Event::NodeFinished { node, outcome: Outcome::Succeeded, .. }) if node.id == "before")));
    assert!(events.iter().any(|event| matches!(&event.payload, StreamPayload::Execution(Event::NodeFinished { node, outcome: Outcome::Failed, .. }) if node.id == "consume")));
    assert!(!events.iter().any(|event| matches!(&event.payload, StreamPayload::Execution(Event::NodeFinished { node, outcome: Outcome::Succeeded, .. }) if node.id == "consume")));
    assert!(events.iter().any(|event| matches!(
        &event.payload,
        StreamPayload::Control(StreamEvent::Finished { failure: Some(failure), .. })
            if failure.node.as_deref() == Some("consume")
    )));

    let value = json!({"items":[null, true, 7]});
    let recording = Snapshots::memory();
    let plan = self::plan(
        json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[{"id":"copy", "kind":"builtin.identity"}],
        "outputs":[{"name":"value","node":"copy","port":"value"}]}),
    );
    let instance = mf_compiler::start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            snapshots: Some(recording.recorder()),
            arguments: mf_runtime::WorkflowArguments::try_from(json!({"copy.input":value}))
                .unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(instance.recv().unwrap().unwrap().outputs["value"], value);
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    recording.finish();
    let latest = recording.current();
    let snapshot = latest.node(&[], "copy").unwrap();
    assert_eq!(snapshot.inputs["input"], value);
    assert_eq!(snapshot.outputs["value"], value);
    assert_eq!(snapshot.outcome, mf_runtime::SnapshotOutcome::Succeeded);
    assert!(recording.diagnostic().is_none());
}

#[test]
fn loop_paths_and_iteration_details_keep_the_containing_message_identity() {
    let value = json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[
            {"id":"repeat", "kind":"workflow.loop", "loop":{"max_iterations":2, "variables":[{"name":"x", "type":"int"}], "body":{
                "nodes":[{"id":"copy", "kind":"builtin.identity"}], "edges":[{"from_node":"%loop", "from_output":"x", "to_node":"copy", "to_input":"input"}]}}},
            {"id":"collect", "kind":"builtin.batch", "config":{"max_items":1, "max_wait_ms":100}},
            {"id":"iterate", "kind":"builtin.iteration", "config":{"body":{"nodes":[{"id":"copy", "kind":"builtin.identity"}],
                "edges":[{"from_node":"%iteration", "from_output":"item", "to_node":"copy", "to_input":"input"}], "result":{"node":"copy", "port":"value"}}}}
        , controlled::source(json!("int"))], "edges":[{"from_node":"feed", "from_output":"item", "to_node":"repeat", "to_input":"x"},
            {"from_node":"repeat", "from_output":"x", "to_node":"collect", "to_input":"item"},
            {"from_node":"collect", "from_output":"items", "to_node":"iterate", "to_input":"items"}],
        "outputs":[{"name":"value", "node":"iterate", "port":"results"}]
    });
    let plan = plan(value);
    let harness = Harness::new(true);
    let observation = plan
        .start_stream_observation(&harness.observer(), RunId::new())
        .unwrap();
    let capture = Snapshots::memory();
    let instance = start_stream(
        &plan,
        &NodeRegistry::from_inventory().unwrap(),
        StreamOptions {
            observation: Some(observation),
            snapshots: Some(capture.recorder()),
            ..Default::default()
        },
    )
    .unwrap();
    instance.source.clone().send(json!(1)).unwrap();
    instance.source.clone().send(json!(2)).unwrap();
    instance.source.close();
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
    capture.finish();
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
    assert!(capture.diagnostic().is_none(), "{:?}", capture.diagnostic());
    let copies = capture
        .history()
        .into_iter()
        .filter(|entry| {
            entry.node.as_ref() == "copy"
                && entry
                    .snapshot
                    .node(&entry.scope, &entry.node)
                    .unwrap()
                    .outcome
                    == mf_runtime::SnapshotOutcome::Succeeded
        })
        .collect::<Vec<_>>();
    assert_eq!(copies.len(), 6);
    assert!(copies.iter().all(|entry| !entry.scope.is_empty()));
    assert_eq!(
        copies
            .iter()
            .map(|entry| entry
                .snapshot
                .node(&entry.scope, &entry.node)
                .unwrap()
                .outputs["value"]
                .as_i64()
                .unwrap())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2])
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
    // Other lifecycle tests decode each event shape; this probe checks sequence capacity.
    let captured = harness.capture.0.lock().unwrap();
    let mut events = captured
        .iter()
        .filter(|record| record.scope == INSTRUMENTATION_SCOPE);
    assert_eq!(events.clone().count(), 40_022);
    let last = StreamRecord::decode(events.next_back().unwrap()).unwrap();
    assert!(last.sequence > mf_telemetry::maximum_loop_event_count());
    let wire = captured.last().unwrap();
    assert_eq!(StreamRecord::decode(wire).unwrap(), last);
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
