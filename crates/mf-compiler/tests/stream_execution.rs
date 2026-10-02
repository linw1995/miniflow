extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{
    Inputs, NodeExecutionError, NodeRegistration, Outputs, PortSpec, StreamClock, StreamError,
    StreamInstance, StreamOptions, ValueType,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    task::Waker,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct ManualClock {
    time: Mutex<Duration>,
    wakers: Mutex<Vec<Waker>>,
}
impl ManualClock {
    fn advance(&self, time: Duration) {
        *self.time.lock().unwrap() = time;
        for waker in self.wakers.lock().unwrap().iter() {
            waker.wake_by_ref();
        }
    }
}
impl StreamClock for ManualClock {
    fn now(&self) -> Duration {
        *self.time.lock().unwrap()
    }
    fn register_waker(&self, waker: Waker) {
        self.wakers.lock().unwrap().push(waker);
    }
}

#[path = "fixtures/event_collector.rs"]
mod event_collector;

struct Counter {
    count: AtomicUsize,
    omit_after_first: bool,
}
impl mf_runtime::TaskNode for Counter {
    fn execute(
        &self,
        _: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let count = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok((if self.omit_after_first && count > 1 {
            Outputs::new()
        } else {
            Outputs::from([("value".into(), json!(count).into())])
        })
        .into())
    }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.counter",
        factory: mf_runtime::NodeFactory::Plain(|config| {
            Ok(mf_runtime::PreparedNode::new(
                Counter {
                    count: AtomicUsize::new(0),
                    omit_after_first: config
                        .get("omit_after_first")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                mf_runtime::NodePorts {
                        inputs: vec![PortSpec::new("input", ValueType::Any, true)],
                        outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
                    },
            ))
        }),
    }
}

#[derive(Default)]
struct Gate {
    started: AtomicUsize,
    open: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct ReleaseGate(Arc<Gate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn gates() -> &'static Mutex<BTreeMap<String, Arc<Gate>>> {
    static GATES: OnceLock<Mutex<BTreeMap<String, Arc<Gate>>>> = OnceLock::new();
    GATES.get_or_init(Mutex::default)
}
struct Slow(Arc<Gate>);
impl mf_runtime::TaskNode for Slow {
    fn execute(
        &self,
        mut inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        self.0.started.fetch_add(1, Ordering::SeqCst);
        let mut open = self.0.open.lock().unwrap();
        while !*open {
            open = self.0.changed.wait(open).unwrap();
        }
        Ok((Outputs::from([("value".into(), inputs.remove("input").unwrap())])).into())
    }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.slow",
        factory: mf_runtime::NodeFactory::Plain(|config| {
            Ok(mf_runtime::PreparedNode::new(
                Slow(Arc::clone(
                    &gates().lock().unwrap()[config["gate"].as_str().unwrap()],
                )),
                mf_runtime::NodePorts {
                        inputs: vec![PortSpec::new("input", ValueType::Any, true)],
                        outputs: vec![PortSpec::new("value", ValueType::Any, true)],
                    },
            ))
        }),
    }
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < limit, "stream did not make progress");
        thread::sleep(Duration::from_millis(1));
    }
}
fn edge(source: &str, port: &str, target: &str, input: &str) -> Value {
    json!({"from_node":source, "from_output":port, "to_node":target, "to_input":input})
}
fn graph(nodes: Value, edges: Vec<Value>, outputs: Value) -> Value {
    json!({"version":"2026-10-02", "execution":{"mode":"stream", "input_type":"int"}, "dependencies":{},
        "nodes":nodes, "edges":edges, "outputs":outputs})
}
fn start(value: Value, clock: Arc<dyn StreamClock>) -> StreamInstance {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            clock,
            ..StreamOptions::default()
        })
        .unwrap()
}
fn accumulating() -> Value {
    graph(
        json!([{"id":"collect", "kind":"test.accumulate"}, {"id":"copy", "kind":"builtin.identity"}]),
        vec![
            edge("%input", "item", "collect", "item"),
            edge("collect", "items", "copy", "input"),
        ],
        json!([{"name":"batch", "node":"copy", "port":"value"}]),
    )
}

#[test]
fn admission_finishes_before_emission_and_idle_timers_wake_the_instance() {
    let clock = Arc::new(ManualClock::default());
    let instance = start(accumulating(), clock.clone());
    let sender = instance.input();
    sender.send(json!(1)).unwrap();
    sender.send(json!(2)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    assert_eq!(instance.summary().emitted_messages, 0);
    clock.advance(Duration::from_millis(100));
    let output = instance.recv().unwrap().unwrap();
    assert_eq!(output.message.domain, 1);
    assert_eq!(output.outputs["batch"], json!([1, 2]));
    sender.close();
    sender.close();
    assert!(matches!(sender.send(json!(3)), Err(StreamError::Closed)));
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().accepted_inputs, 2);
}

#[test]
fn instances_own_their_nodes_and_frames_do_not_reuse_omitted_outputs() {
    let definition: WorkflowDefinition = serde_json::from_value(graph(
        json!([{"id":"count", "kind":"test.counter"}]),
        vec![edge("%input", "item", "count", "input")],
        json!([{"name":"count", "node":"count", "port":"value"}]),
    ))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    for _ in 0..2 {
        let instance = instantiate_stream(&plan, &registry)
            .unwrap()
            .start()
            .unwrap();
        instance.input().send(json!(0)).unwrap();
        instance.close_input();
        assert_eq!(instance.recv().unwrap().unwrap().outputs["count"], json!(1));
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
    }
    let mut value = serde_json::to_value(definition).unwrap();
    value["nodes"][0]["config"] = json!({"omit_after_first":true});
    let instance = start(value, Arc::new(ManualClock::default()));
    instance.input().send(json!(0)).unwrap();
    instance.input().send(json!(0)).unwrap();
    instance.close_input();
    assert_eq!(instance.recv().unwrap().unwrap().outputs["count"], json!(1));
    assert!(
        instance
            .recv()
            .unwrap_err()
            .to_string()
            .contains("count.value")
    );
    assert!(instance.join().is_err());
}

#[test]
fn blocking_business_work_does_not_stop_input_or_timer_progress() {
    let gate = Arc::new(Gate::default());
    gates()
        .lock()
        .unwrap()
        .insert("timer-progress".into(), Arc::clone(&gate));
    let mut value = accumulating();
    value["nodes"][1] =
        json!({"id":"copy", "kind":"test.slow", "config":{"gate":"timer-progress"}});
    let clock = Arc::new(ManualClock::default());
    let instance = start(value, clock.clone());
    let _release = ReleaseGate(Arc::clone(&gate));
    instance.input().send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 1);
    clock.advance(Duration::from_millis(100));
    wait_until(|| gate.started.load(Ordering::SeqCst) == 1);
    instance.input().send(json!(2)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    clock.advance(Duration::from_millis(200));
    wait_until(|| instance.summary().emitted_messages == 2);
    gate.release();
    instance.close_input();
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([1])
    );
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["batch"],
        json!([2])
    );
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    gates().lock().unwrap().remove("timer-progress");
}

#[test]
fn worker_concurrency_is_bounded_across_independent_domains() {
    let gate = Arc::new(Gate::default());
    gates()
        .lock()
        .unwrap()
        .insert("worker-limit".into(), Arc::clone(&gate));
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for index in 0..3 {
        let collect = format!("collect_{index}");
        let slow = format!("slow_{index}");
        nodes.push(json!({"id":collect, "kind":"test.accumulate"}));
        nodes.push(json!({"id":slow, "kind":"test.slow", "config":{"gate":"worker-limit"}}));
        edges.push(edge("%input", "item", &collect, "item"));
        edges.push(edge(&collect, "items", &slow, "input"));
    }
    let mut value = graph(json!(nodes), edges, json!([]));
    value["execution"]["limits"] = json!({"workers":2});
    let clock = Arc::new(ManualClock::default());
    let instance = start(value, clock.clone());
    let _release = ReleaseGate(Arc::clone(&gate));
    instance.input().send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 1);
    clock.advance(Duration::from_millis(100));
    wait_until(|| gate.started.load(Ordering::SeqCst) == 2);
    assert_eq!(instance.summary().emitted_messages, 3);
    assert_eq!(gate.started.load(Ordering::SeqCst), 2);
    gate.release();
    instance.close_input();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    assert_eq!(gate.started.load(Ordering::SeqCst), 3);
    gates().lock().unwrap().remove("worker-limit");
}

#[test]
fn per_message_budgets_allow_a_long_lived_instance_and_preserve_fifo() {
    let value = graph(
        json!([{"id":"copy", "kind":"builtin.identity"}]),
        vec![edge("%input", "item", "copy", "input")],
        json!([{"name":"value", "node":"copy", "port":"value"}]),
    );
    let instance = start(value, Arc::new(ManualClock::default()));
    let count = mf_runtime::MAX_SCHEDULED_STEPS + 1;
    thread::scope(|scope| {
        let sender = instance.input();
        let producer = scope.spawn(move || {
            for value in 0..count {
                sender.send(json!(value)).unwrap();
            }
            sender.close();
        });
        for expected in 0..count {
            let output = instance.recv().unwrap().unwrap();
            assert_eq!(output.message.sequence, expected as u64);
            assert_eq!(output.outputs["value"], json!(expected));
        }
        assert!(instance.recv().unwrap().is_none());
        producer.join().unwrap();
    });
    assert_eq!(instance.join().unwrap().completed_frames, count as u64);
}

#[test]
fn invalid_input_and_empty_close_are_explicit() {
    let instance = start(accumulating(), Arc::new(ManualClock::default()));
    assert!(matches!(
        instance.input().send(json!("invalid")),
        Err(StreamError::Input { .. })
    ));
    assert_eq!(instance.summary().accepted_inputs, 0);
    assert!(instance.join().is_err());
    let instance = start(accumulating(), Arc::new(ManualClock::default()));
    instance.close_input();
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().emitted_messages, 0);
}

struct Exhaust;
struct Empty;
impl mf_runtime::TaskNode for Empty {
    fn execute(
        &self,
        _: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::new()).into())
    }
}
impl mf_runtime::TaskNode for Exhaust {
    fn execute(
        &self,
        _: Inputs,
        context: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let inner = mf_runtime::FlowNode::new(
            "inner",
            mf_runtime::PreparedNode::new(Empty, mf_runtime::NodePorts::default()),
        )
        .into_task()
        .unwrap();
        for _ in 0..mf_runtime::MAX_SCHEDULED_STEPS {
            mf_runtime::execute_node_in_context(&inner, &[], context).map_err(|error| {
                NodeExecutionError::ExecutionFailed {
                    message: error.to_string(),
                }
            })?;
        }
        Ok(mf_runtime::NodeResult::default())
    }
}
inventory::submit! {
    NodeRegistration {
        kind: "test.exhaust",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            Ok(mf_runtime::PreparedNode::new(
                Exhaust,
                mf_runtime::NodePorts {
                        inputs: vec![PortSpec::new("input", ValueType::Any, true)],
                        outputs: vec![],
                    },
            ))
        }),
    }
}

#[test]
fn one_over_budget_frame_fails_and_cancellation_wakes_an_idle_instance() {
    let value = graph(
        json!([{"id":"exhaust", "kind":"test.exhaust"}]),
        vec![edge("%input", "item", "exhaust", "input")],
        json!([]),
    );
    let instance = start(value, Arc::new(ManualClock::default()));
    instance.input().send(json!(1)).unwrap();
    instance.close_input();
    let error = instance.recv().unwrap_err().to_string();
    assert!(
        error.contains("budget") && error.contains("message 0"),
        "{error}"
    );
    assert!(instance.join().is_err());

    let instance = start(accumulating(), Arc::new(ManualClock::default()));
    instance.cancel();
    assert!(matches!(instance.recv(), Err(StreamError::Cancelled)));
    assert!(matches!(instance.join(), Err(StreamError::Cancelled)));
}
