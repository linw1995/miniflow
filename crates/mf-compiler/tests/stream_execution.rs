#[path = "fixtures/controlled_source.rs"]
mod controlled;
use controlled::SourceRun;
extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{
    Inputs, NodeExecutionError, NodeRegistration, Outputs, PortSpec, StreamClock, StreamOptions,
    ValueType,
};
use serde_json::{Value, json};
use snafu::ResultExt;
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
fn graph(mut nodes: Value, edges: Vec<Value>, outputs: Value) -> Value {
    nodes
        .as_array_mut()
        .unwrap()
        .push(controlled::source(json!("int")));
    json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":nodes, "edges":edges, "outputs":outputs})
}
fn start(value: Value, clock: Arc<dyn StreamClock>) -> SourceRun {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    SourceRun::start(
        instantiate_stream(&plan, &registry).unwrap(),
        "feed",
        StreamOptions {
            clock,
            ..Default::default()
        },
    )
    .unwrap()
}
fn accumulating() -> Value {
    graph(
        json!([{"id":"collect", "kind":"test.accumulate"}, {"id":"copy", "kind":"builtin.identity"}]),
        vec![
            edge("feed", "item", "collect", "item"),
            edge("collect", "items", "copy", "input"),
        ],
        json!([{"name":"batch", "node":"copy", "port":"value"}]),
    )
}

#[test]
fn idle_timers_flush_while_a_source_remains_open() {
    let clock = Arc::new(ManualClock::default());
    let instance = start(accumulating(), clock.clone());
    let sender = instance.source.clone();
    sender.send(json!(1)).unwrap();
    sender.send(json!(2)).unwrap();
    wait_until(|| instance.summary().completed_frames == 3);
    assert_eq!(instance.summary().emitted_messages, 2);
    clock.advance(Duration::from_millis(100));
    let output = instance.recv().unwrap().unwrap();
    assert_eq!(output.message.domain, 2);
    assert_eq!(output.outputs["batch"], json!([1, 2]));
    sender.close();
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().startup_frames, 1);
}

#[test]
fn instances_own_their_nodes_and_frames_do_not_reuse_omitted_outputs() {
    let definition: WorkflowDefinition = serde_json::from_value(graph(
        json!([{"id":"count", "kind":"test.counter"}]),
        vec![edge("feed", "item", "count", "input")],
        json!([{"name":"count", "node":"count", "port":"value"}]),
    ))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    for _ in 0..2 {
        let instance = SourceRun::start(
            instantiate_stream(&plan, &registry).unwrap(),
            "feed",
            StreamOptions::default(),
        )
        .unwrap();
        instance.source.clone().send(json!(0)).unwrap();
        instance.source.close();
        assert_eq!(instance.recv().unwrap().unwrap().outputs["count"], json!(1));
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
    }
    let mut value = serde_json::to_value(definition).unwrap();
    value["nodes"][0]["config"] = json!({"omit_after_first":true});
    let instance = start(value, Arc::new(ManualClock::default()));
    instance.source.clone().send(json!(0)).unwrap();
    instance.source.clone().send(json!(0)).unwrap();
    instance.source.close();
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
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"before", "kind":"builtin.identity"}),
        json!({"id":"after", "kind":"builtin.identity"}),
    ]);
    value["edges"] = json!([
        edge("feed", "item", "collect", "item"),
        edge("collect", "items", "before", "input"),
        edge("before", "value", "copy", "input"),
        edge("copy", "value", "after", "input"),
    ]);
    value["outputs"][0]["node"] = json!("after");
    let clock = Arc::new(ManualClock::default());
    let instance = start(value, clock.clone());
    let _release = ReleaseGate(Arc::clone(&gate));
    instance.source.clone().send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    clock.advance(Duration::from_millis(100));
    wait_until(|| gate.started.load(Ordering::SeqCst) == 1);
    instance.source.clone().send(json!(2)).unwrap();
    wait_until(|| instance.summary().completed_frames == 3);
    clock.advance(Duration::from_millis(200));
    wait_until(|| instance.summary().emitted_messages == 4);
    gate.release();
    instance.source.close();
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
        edges.push(edge("feed", "item", &collect, "item"));
        edges.push(edge(&collect, "items", &slow, "input"));
    }
    let mut value = graph(json!(nodes), edges, json!([]));
    value["execution"]["limits"] = json!({"workers":2});
    let clock = Arc::new(ManualClock::default());
    let instance = start(value, clock.clone());
    let _release = ReleaseGate(Arc::clone(&gate));
    instance.source.clone().send(json!(1)).unwrap();
    wait_until(|| instance.summary().completed_frames == 2);
    clock.advance(Duration::from_millis(100));
    wait_until(|| gate.started.load(Ordering::SeqCst) == 2);
    assert_eq!(instance.summary().emitted_messages, 4);
    assert_eq!(gate.started.load(Ordering::SeqCst), 2);
    gate.release();
    instance.source.close();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    assert_eq!(gate.started.load(Ordering::SeqCst), 3);
    gates().lock().unwrap().remove("worker-limit");
}

#[test]
fn per_message_budgets_allow_a_long_lived_instance_and_preserve_fifo() {
    let mut value = graph(
        json!([
            {"id":"left", "kind":"builtin.identity"},
            {"id":"right", "kind":"builtin.identity"},
            {"id":"copy", "kind":"builtin.identity"},
        ]),
        vec![
            edge("feed", "item", "left", "input"),
            edge("feed", "item", "right", "input"),
            edge("left", "value", "copy", "input"),
        ],
        json!([{"name":"value", "node":"copy", "port":"value"}]),
    );
    value["control_edges"] = json!([
        {"from_node":"right", "from_output":"value", "to_node":"copy"}
    ]);
    let instance = start(value, Arc::new(ManualClock::default()));
    // Each message schedules left, right, and copy; exceed the legacy run-wide budget.
    let count = mf_runtime::MAX_SCHEDULED_STEPS / 3 + 1;
    thread::scope(|scope| {
        let sender = instance.source.clone();
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
    assert_eq!(instance.join().unwrap().completed_frames, count as u64 + 1);
}

#[test]
fn empty_source_closure_does_not_emit() {
    let instance = start(accumulating(), Arc::new(ManualClock::default()));
    instance.source.close();
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
            mf_runtime::execute_node_in_context(&inner, &[], context)
                .boxed()
                .context(mf_runtime::NodePluginFailedSnafu)?;
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
fn one_over_budget_frame_fails() {
    let value = graph(
        json!([{"id":"exhaust", "kind":"test.exhaust"}]),
        vec![edge("feed", "item", "exhaust", "input")],
        json!([]),
    );
    let instance = start(value, Arc::new(ManualClock::default()));
    instance.source.clone().send(json!(1)).unwrap();
    instance.source.close();
    let error = instance.recv().unwrap_err().to_string();
    assert!(
        error.contains("budget") && error.contains("message 0"),
        "{error}"
    );
    assert!(instance.join().is_err());
}
