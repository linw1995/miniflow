#[path = "fixtures/event_collector.rs"]
mod event_collector;

extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{
    Inputs, NodeExecutionError, NodeRegistration, Outputs, PortSpec, PreparedStream, StreamClock,
    StreamError, StreamInstance, StreamMetrics, StreamOptions, ValueRef, ValueType,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    task::Waker,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Clock {
    now: Mutex<Duration>,
    wakers: Mutex<Vec<Waker>>,
}
impl StreamClock for Clock {
    fn now(&self) -> Duration {
        *self.now.lock().unwrap()
    }
    fn register_waker(&self, waker: Waker) {
        self.wakers.lock().unwrap().push(waker);
    }
}
impl Clock {
    fn advance(&self, milliseconds: u64) {
        *self.now.lock().unwrap() = Duration::from_millis(milliseconds);
        for waker in self.wakers.lock().unwrap().iter() {
            waker.wake_by_ref();
        }
    }
}

#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    drops: AtomicUsize,
    open: Mutex<bool>,
    changed: Condvar,
}
impl Probe {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct Release(Arc<Probe>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
fn probes() -> &'static Mutex<BTreeMap<String, Arc<Probe>>> {
    static PROBES: OnceLock<Mutex<BTreeMap<String, Arc<Probe>>>> = OnceLock::new();
    PROBES.get_or_init(Mutex::default)
}
struct Sink {
    probe: Arc<Probe>,
    block: bool,
    fail_at: Option<i64>,
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.probe.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl mf_runtime::TaskNode for Sink {
    fn execute(
        &self,
        mut inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        self.probe.calls.fetch_add(1, Ordering::SeqCst);
        if self.block {
            let mut open = self.probe.open.lock().unwrap();
            while !*open {
                open = self.probe.changed.wait(open).unwrap();
            }
        }
        let value = inputs.remove("input").unwrap();
        let first = value
            .as_array()
            .and_then(|items| items.first())
            .and_then(ValueRef::as_i64);
        if first.is_some() && first == self.fail_at {
            return Err(NodeExecutionError::ExecutionFailed {
                message: "deliberate batch failure".into(),
            });
        }
        Ok((Outputs::from([("value".into(), value)])).into())
    }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.capacity_sink",
        factory: mf_runtime::NodeFactory::Plain(|config| {
            Ok(mf_runtime::PreparedNode::new(
                Sink {
                    probe: Arc::clone(&probes().lock().unwrap()[config["key"].as_str().unwrap()]),
                    block: config["block"].as_bool().unwrap_or(false),
                    fail_at: config["fail_at"].as_i64(),
                },
                mf_runtime::NodePorts {
                        inputs: vec![PortSpec::new("input", ValueType::Any, true)],
                        outputs: vec![PortSpec::new("value", ValueType::Any, true)],
                    },
            ))
        }),
    }
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "stream did not make progress");
        thread::sleep(Duration::from_millis(1));
    }
}
fn definition() -> Value {
    json!({
      "version": "2026-10-02",
      "execution": { "mode": "stream", "input_type": "int" },
      "dependencies": {
        "core": { "package": "mfn-core", "path": "../crates/builtin-nodes/core" }
      },
      "nodes": [
        {
          "id": "collect",
          "kind": "test.accumulate",
          "config": { "max_items": 3, "max_wait_ms": 250 }
        },
        { "id": "consume", "kind": "builtin.identity" }
      ],
      "edges": [
        { "from_node": "%input", "from_output": "item", "to_node": "collect", "to_input": "item" },
        { "from_node": "collect", "from_output": "items", "to_node": "consume", "to_input": "input" }
      ],
      "outputs": [{ "name": "batch", "node": "consume", "port": "value" }]
    })
}
fn prepare(value: Value) -> Result<PreparedStream, mf_compiler::WorkflowCompileError> {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry)?;
    instantiate_stream(&plan, &registry)
}
fn start(value: Value, clock: Arc<Clock>) -> StreamInstance {
    prepare(value)
        .unwrap()
        .start_with_options(StreamOptions {
            clock,
            ..StreamOptions::default()
        })
        .unwrap()
}

#[test]
fn a_slow_consumer_backpressures_admission_with_bounded_frames() {
    let mut value = definition();
    value["execution"]["limits"] = json!({"max_pending_messages":2});
    value["nodes"][0]["config"]["max_items"] = json!(1);
    let instance = start(value, Arc::new(Clock::default()));
    let input = instance.input();
    input.send(json!(0)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    let mut accepted = 1;
    loop {
        assert!(accepted < 100, "output pressure did not reach admission");
        let item: ValueRef = json!(accepted).into();
        match input.try_send(&item) {
            Ok(()) => accepted += 1,
            Err(StreamError::Capacity) => break,
            Err(error) => panic!("unexpected admission failure: {error}"),
        }
    }
    let metrics = instance.metrics();
    assert!(metrics.pending_frames <= 2, "{metrics:?}");
    input.close();
    for expected in 0..accepted {
        assert_eq!(
            instance.recv().unwrap().unwrap().outputs["batch"],
            json!([expected])
        );
        assert!(instance.metrics().pending_frames <= 2);
    }
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.metrics(), StreamMetrics::default());
    assert_eq!(instance.join().unwrap().accepted_inputs, accepted);
}

#[test]
fn timeouts_make_progress_when_the_count_threshold_exceeds_frame_capacity() {
    let mut value = definition();
    value["execution"]["limits"] = json!({"max_pending_messages":2});
    value["nodes"][0]["config"] = json!({"max_items":100, "max_wait_ms":100});
    let clock = Arc::new(Clock::default());
    let instance = start(value, clock.clone());
    for value in 0..5 {
        instance.input().send(json!(value)).unwrap();
    }
    wait_until(|| instance.summary().completed_frames == 5);
    clock.advance(100);
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([0, 1, 2, 3, 4])
    );
    instance.close_input();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}

#[test]
fn upstream_close_waits_for_work_already_running_before_flushing_the_tail() {
    let probe = Arc::new(Probe::default());
    probes()
        .lock()
        .unwrap()
        .insert("close-upstream".into(), Arc::clone(&probe));
    let mut value = definition();
    value["nodes"][0]["config"]["max_items"] = json!(100);
    value["nodes"].as_array_mut().unwrap().push(json!({"id":"slow", "kind":"test.capacity_sink", "config":{"key":"close-upstream", "block":true}}));
    value["edges"][0]["from_node"] = json!("slow");
    value["edges"][0]["from_output"] = json!("value");
    value["edges"].as_array_mut().unwrap().push(
        json!({"from_node":"%input", "from_output":"item", "to_node":"slow", "to_input":"input"}),
    );
    let instance = start(value, Arc::new(Clock::default()));
    let _release = Release(Arc::clone(&probe));
    instance.input().send(json!(1)).unwrap();
    wait_until(|| probe.calls.load(Ordering::SeqCst) == 1);
    let mut accepted = 1;
    loop {
        let item = json!(accepted + 1).into();
        match instance.input().try_send(&item) {
            Ok(()) => accepted += 1,
            Err(StreamError::Capacity) => break,
            Err(error) => panic!("unexpected admission failure: {error}"),
        }
    }
    instance.close_input();
    assert_eq!(instance.summary().emitted_messages, 0);
    probe.release();
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!((1..=accepted).collect::<Vec<_>>())
    );
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    probes().lock().unwrap().remove("close-upstream");
}

#[test]
fn chained_collectors_seal_all_tails_before_output_is_consumed() {
    let mut value = definition();
    value["nodes"][0]["config"]["max_items"] = json!(2);
    value["nodes"][1] = json!({"id":"consume", "kind":"test.accumulate", "config":{"max_items":2, "max_wait_ms":100}});
    value["edges"][1]["to_input"] = json!("item");
    value["outputs"][0]["port"] = json!("items");
    let instance = start(value, Arc::new(Clock::default()));
    for value in 1..=5 {
        instance.input().send(json!(value)).unwrap();
    }
    instance.close_input();
    wait_until(|| instance.summary().emitted_messages == 5);
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([[1, 2], [3, 4]])
    );
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([[5]])
    );
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}

#[test]
fn input_failure_discards_a_partial_buffer_and_wakes_a_blocked_producer() {
    let clock = Arc::new(Clock::default());
    let instance = start(definition(), clock);
    let input = instance.input();
    input.send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 1);
    assert!(matches!(
        instance.input().send(json!("invalid")),
        Err(StreamError::Input { .. })
    ));
    assert!(matches!(instance.recv(), Err(StreamError::Input { .. })));
    wait_until(|| instance.metrics() == StreamMetrics::default());
    assert_eq!(instance.summary().emitted_messages, 0);
    assert!(matches!(instance.join(), Err(StreamError::Input { .. })));
    assert!(matches!(
        input.send(json!(2)),
        Err(StreamError::Input { .. })
    ));

    let mut value = definition();
    value["execution"]["limits"] = json!({"max_pending_messages":2});
    value["nodes"][0]["config"]["max_items"] = json!(1);
    let instance = start(value, Arc::new(Clock::default()));
    let input = instance.input();
    input.send(json!(0)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    input.send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 3);
    input.send(json!(2)).unwrap();
    assert!(matches!(
        input.try_send(&json!(3).into()),
        Err(StreamError::Capacity)
    ));
    thread::scope(|scope| {
        let producer = scope.spawn(move || input.send(json!(3)));
        assert!(matches!(
            instance.input().send(json!("invalid")),
            Err(StreamError::Input { .. })
        ));
        assert!(matches!(
            producer.join().unwrap(),
            Err(StreamError::Input { .. })
        ));
    });
    assert!(instance.join().is_err());
}

#[test]
fn a_later_failure_preserves_the_delivered_prefix_and_never_retries() {
    let probe = Arc::new(Probe::default());
    probes()
        .lock()
        .unwrap()
        .insert("fail-prefix".into(), Arc::clone(&probe));
    let mut value = definition();
    value["nodes"][0]["config"]["max_items"] = json!(1);
    value["nodes"][1] = json!({"id":"consume", "kind":"test.capacity_sink", "config":{"key":"fail-prefix", "fail_at":2}});
    let instance = start(value, Arc::new(Clock::default()));
    for value in 1..=3 {
        instance.input().send(json!(value)).unwrap();
    }
    instance.close_input();
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([1])
    );
    let error = instance.recv().unwrap_err();
    assert!(error.to_string().contains("deliberate batch failure"));
    let cause = error.source().unwrap();
    assert!(cause.is::<Arc<mf_runtime::WorkflowRunError>>());
    assert!(cause.source().unwrap().is::<NodeExecutionError>());
    let input = instance.input();
    let StreamError::Workflow { source, message } = error else {
        panic!("expected a workflow error");
    };
    for error in [
        input.send(json!(4)).unwrap_err(),
        instance.join().unwrap_err(),
    ] {
        let StreamError::Workflow {
            source: shared,
            message: failed_message,
        } = error
        else {
            panic!("expected the same terminal error");
        };
        assert!(Arc::ptr_eq(&source, &shared));
        assert_eq!(message, failed_message);
    }
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    probes().lock().unwrap().remove("fail-prefix");
}

#[test]
fn plugin_objects_are_released_even_when_a_sender_handle_outlives_the_instance() {
    let probe = Arc::new(Probe::default());
    probes()
        .lock()
        .unwrap()
        .insert("release-nodes".into(), Arc::clone(&probe));
    let mut value = definition();
    value["nodes"][1] =
        json!({"id":"consume", "kind":"test.capacity_sink", "config":{"key":"release-nodes"}});
    let instance = start(value.clone(), Arc::new(Clock::default()));
    let before = probe.drops.load(Ordering::SeqCst);
    let input = instance.input();
    input.send(json!(1)).unwrap();
    input.close();
    assert!(instance.recv().unwrap().is_some());
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    assert_eq!(probe.drops.load(Ordering::SeqCst), before + 1);
    assert!(matches!(input.send(json!(2)), Err(StreamError::Closed)));
    let instance = start(value, Arc::new(Clock::default()));
    let before = probe.drops.load(Ordering::SeqCst);
    let input = instance.input();
    input.send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 1);
    drop(instance);
    assert_eq!(probe.drops.load(Ordering::SeqCst), before + 1);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        input.send(json!(2)),
        Err(StreamError::Execution { .. })
    ));
    probes().lock().unwrap().remove("release-nodes");
}

#[test]
fn rejects_snapshot_capture_and_insufficient_domain_capacity() {
    let error = prepare(definition())
        .unwrap()
        .start_with_options(StreamOptions {
            snapshots: Some(mf_runtime::SnapshotRecorder::memory()),
            ..Default::default()
        })
        .err()
        .unwrap();
    assert!(error.to_string().contains("snapshot capture"));

    let mut value = definition();
    value["execution"]["limits"] = json!({"max_pending_messages":1});
    let error = prepare(value).unwrap_err();
    assert!(error.to_string().contains("reserve"), "{error}");
    assert!(error.source().unwrap().is::<mf_runtime::StreamBuildError>());
}

#[test]
fn input_failure_suppresses_followup_work_after_a_running_call_returns() {
    let running = Arc::new(Probe::default());
    let following = Arc::new(Probe::default());
    probes()
        .lock()
        .unwrap()
        .insert("late-running".into(), Arc::clone(&running));
    probes()
        .lock()
        .unwrap()
        .insert("late-following".into(), Arc::clone(&following));
    let mut value = definition();
    value["nodes"][0]["config"]["max_items"] = json!(1);
    value["nodes"][1] = json!({"id":"consume", "kind":"test.capacity_sink", "config":{"key":"late-running", "block":true}});
    value["nodes"].as_array_mut().unwrap().push(
        json!({"id":"following", "kind":"test.capacity_sink", "config":{"key":"late-following"}}),
    );
    value["edges"].as_array_mut().unwrap().push(json!({"from_node":"consume", "from_output":"value", "to_node":"following", "to_input":"input"}));
    value["outputs"][0]["node"] = json!("following");
    let instance = start(value, Arc::new(Clock::default()));
    let _release = Release(Arc::clone(&running));
    instance.input().send(json!(1)).unwrap();
    wait_until(|| running.calls.load(Ordering::SeqCst) == 1);
    instance.input().send(json!(2)).unwrap();
    wait_until(|| instance.summary().emitted_messages == 2);
    assert!(matches!(
        instance.input().send(json!("invalid")),
        Err(StreamError::Input { .. })
    ));
    assert!(matches!(instance.recv(), Err(StreamError::Input { .. })));
    assert_eq!(instance.metrics().active_workers, 1);
    running.release();
    assert!(matches!(instance.join(), Err(StreamError::Input { .. })));
    assert_eq!(running.calls.load(Ordering::SeqCst), 1);
    assert_eq!(following.calls.load(Ordering::SeqCst), 0);
    probes().lock().unwrap().remove("late-running");
    probes().lock().unwrap().remove("late-following");
}

#[test]
fn final_output_failure_is_reported_before_the_instance_can_complete() {
    let instance = start(definition(), Arc::new(Clock::default()));
    instance.input().send(json!(1)).unwrap();
    instance.close_input();
    let delivery = instance.receive().unwrap().unwrap();
    assert_eq!(delivery.output().outputs["batch"], json!([1]));
    assert_eq!(instance.summary().delivered_outputs, 0);
    assert_eq!(instance.metrics().pending_frames, 1);
    delivery.fail(StreamError::Output {
        message: "sink rejected the final record".into(),
    });
    assert!(matches!(instance.join(), Err(StreamError::Output { .. })));

    let instance = start(definition(), Arc::new(Clock::default()));
    instance.input().send(json!(2)).unwrap();
    instance.close_input();
    drop(instance.receive().unwrap().unwrap());
    assert!(
        instance
            .join()
            .unwrap_err()
            .to_string()
            .contains("before acknowledgement")
    );
}
