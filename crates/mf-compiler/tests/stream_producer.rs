#[path = "fixtures/controlled_source.rs"]
mod controlled;
use controlled::SourceRun;
#[path = "fixtures/observation_capture.rs"]
mod capture;

extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{
    Emitter, ExecutionContext, Flow, FlowNode, Inputs, NodeExecutionError, NodeFactory, NodePorts,
    NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode, StreamInstance, StreamNode,
    StreamOptions, TaskNode, ValueType,
};
use mf_telemetry::{
    event::{Event, FailurePhase, Outcome},
    identity::RunId,
    stream::{StreamPayload, StreamRecord},
};
use opentelemetry::trace::TraceContextExt;
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    attempted: AtomicUsize,
    sent: AtomicUsize,
    dropped: AtomicUsize,
    threads: Mutex<Vec<ThreadId>>,
    spans: Mutex<Vec<opentelemetry::trace::SpanId>>,
    open: Mutex<bool>,
    changed: Condvar,
}

impl Probe {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
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

fn probe(key: &str) -> Arc<Probe> {
    Arc::clone(probes().lock().unwrap().entry(key.into()).or_default())
}

struct Producer {
    count: usize,
    mode: String,
    calls: Cell<i64>,
    probe: Arc<Probe>,
}

fn failure(message: &str) -> NodeExecutionError {
    NodeExecutionError::ExecutionFailed {
        message: message.into(),
    }
}

impl StreamNode for Producer {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        self.calls.set(self.calls.get() + 1);
        self.probe.calls.fetch_add(1, Ordering::SeqCst);
        self.probe
            .threads
            .lock()
            .unwrap()
            .push(thread::current().id());
        self.probe.spans.lock().unwrap().push(
            opentelemetry::Context::current()
                .span()
                .span_context()
                .span_id(),
        );
        if self.mode == "panic" {
            panic!("producer panic sentinel");
        }
        let base = inputs["input"].as_i64().unwrap();
        for item in 0..self.count {
            let value = if self.mode == "invalid" {
                json!("invalid")
            } else if self.mode == "state" {
                json!(self.calls.get())
            } else if self.mode == "context" {
                match context.output("copy.value").unwrap() {
                    mf_runtime::ContextValue::Value(value) => serde_json::to_value(value).unwrap(),
                    _ => panic!("expected context output"),
                }
            } else {
                json!(base + item as i64)
            };
            let result = Outputs::from([("value".into(), value.into())]).into();
            self.probe.attempted.fetch_add(1, Ordering::SeqCst);
            let sent = emitter.send(result);
            if self.mode == "invalid" {
                assert!(sent.is_err());
                return Ok(());
            }
            sent?;
            self.probe.sent.fetch_add(1, Ordering::SeqCst);
            if self.mode == "gate" {
                self.probe.wait();
            }
        }
        if self.mode == "fail_after" {
            self.probe.wait();
            return Err(failure("producer failure sentinel"));
        }
        Ok(())
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.probe.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn producer(config: Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    let key = config["key"].as_str().unwrap();
    let mode = config["mode"].as_str().unwrap_or("range");
    Ok(PreparedNode::stream(
        Producer {
            count: config["count"].as_u64().unwrap_or(1) as usize,
            mode: mode.into(),
            calls: Cell::new(0),
            probe: probe(key),
        },
        mf_runtime::NodeMetadata {
            context_references: if mode == "context" {
                vec![mf_runtime::ContextReference::new(
                    "copy.value",
                    "producer context",
                )]
            } else {
                Vec::new()
            },
            ..mf_runtime::NodeMetadata::new(NodePorts {
                inputs: vec![PortSpec::new("input", ValueType::Int64, true)],
                outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            })
        },
    ))
}

inventory::submit! {
    NodeRegistration { kind: "test.producer", factory: NodeFactory::Plain(producer) }
}

struct Sink(Arc<Probe>);
impl TaskNode for Sink {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        self.0.wait();
        Err(failure("downstream failure sentinel"))
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "test.producer_sink",
        factory: NodeFactory::Plain(|config| Ok(PreparedNode::new(
            Sink(probe(config["key"].as_str().unwrap())),
            NodePorts {
                inputs: vec![PortSpec::new("input", ValueType::Int64, true)],
                outputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            },
        ))),
    }
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "producer did not make progress");
        thread::sleep(Duration::from_millis(1));
    }
}

fn graph(key: &str, count: usize) -> Value {
    json!({
            "version":"2026-10-03", "dependencies":{},
            "execution":{"mode":"stream", "limits":{"max_pending_messages":3, "workers":1}},
            "nodes":[
                {"id":"produce", "kind":"test.producer", "config":{"key":key, "count":count}},
                {"id":"consume", "kind":"builtin.identity"}
            , controlled::source(json!("int"))
    ],
            "edges":[
                {"from_node":"feed", "from_output":"item", "to_node":"produce", "to_input":"input"},
                {"from_node":"produce", "from_output":"value", "to_node":"consume", "to_input":"input"}
            ],
            "outputs":[{"name":"value", "node":"consume", "port":"value"}]
        })
}

fn start(value: Value, options: StreamOptions) -> SourceRun {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    SourceRun::start(
        instantiate_stream(&plan, &registry).unwrap(),
        "feed",
        options,
    )
    .unwrap()
}

fn receive(instance: &StreamInstance) -> i64 {
    instance.recv().unwrap().unwrap().outputs["value"]
        .as_i64()
        .unwrap()
}

#[test]
fn large_output_drains_after_input_closes_with_one_task_worker() {
    let instance = start(graph("large", 1_000), StreamOptions::default());
    instance.source.clone().send(json!(10)).unwrap();
    instance.source.close();
    for expected in 10..1_010 {
        assert_eq!(receive(&instance), expected);
    }
    assert!(instance.recv().unwrap().is_none());
    let summary = instance.join().unwrap();
    assert_eq!(summary.startup_frames, 1);
    assert_eq!(summary.emitted_messages, 1_001);
    assert_eq!(summary.delivered_outputs, 1_000);
}

#[test]
fn stalled_delivery_bounds_sends_and_drop_wakes_the_producer() {
    let instance = start(graph("blocked", 1_000), StreamOptions::default());
    let probe = probe("blocked");
    probe.dropped.store(0, Ordering::SeqCst);
    instance.source.clone().send(json!(0)).unwrap();
    let delivery = instance.receive().unwrap().unwrap();
    wait_until(|| probe.attempted.load(Ordering::SeqCst) == 5);
    assert_eq!(probe.sent.load(Ordering::SeqCst), 4);
    assert_eq!(instance.summary().emitted_messages, 5);
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        drop(instance);
        sender.send(()).unwrap();
    });
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
    drop(delivery);
}

#[test]
fn producer_state_and_worker_are_reused_but_instances_are_isolated() {
    let mut value = graph("state", 1);
    value["nodes"][0]["config"]["mode"] = json!("state");
    let first = start(value.clone(), StreamOptions::default());
    let second = start(value, StreamOptions::default());
    for expected in 1..=3 {
        first.source.clone().send(json!(0)).unwrap();
        assert_eq!(receive(&first), expected);
    }
    second.source.clone().send(json!(0)).unwrap();
    assert_eq!(receive(&second), 1);
    for instance in [first, second] {
        instance.source.close();
        assert!(instance.recv().unwrap().is_none());
        instance.join().unwrap();
    }
    let probe = probe("state");
    let threads = probe.threads.lock().unwrap();
    assert!(threads[..3].iter().all(|id| *id == threads[0]));
    assert_ne!(threads[0], threads[3]);
}

#[test]
fn skips_and_chained_producers_preserve_input_order() {
    let mut value = graph("chain-first", 3);
    value["execution"]["limits"]["max_pending_messages"] = json!(4);
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id":"expand", "kind":"test.producer", "config":{"key":"chain-second", "count":2}
    }));
    value["edges"][1]["to_node"] = json!("expand");
    value["edges"].as_array_mut().unwrap().push(json!({
        "from_node":"expand", "from_output":"value", "to_node":"consume", "to_input":"input"
    }));
    value["nodes"].as_array_mut().unwrap().push(json!({
        "id":"route", "kind":"builtin.if_else", "config":{"branches":[{
            "id":"positive", "condition":{"source":{"output":"feed.item", "path":""}, "operator":"gt", "value":0}
        }]}
    }));
    value["control_edges"] = json!([
        {"from_node":"feed", "from_output":"item", "to_node":"route"},
        {"from_node":"route", "from_output":"positive", "to_node":"produce"}
    ]);
    let instance = start(value, StreamOptions::default());
    let input = instance.source.clone();
    let producer = thread::spawn(move || {
        for value in [0, 10, 20] {
            input.send(json!(value)).unwrap();
        }
        input.close();
    });
    for expected in [10, 11, 11, 12, 12, 13, 20, 21, 21, 22, 22, 23] {
        assert_eq!(receive(&instance), expected);
    }
    assert!(instance.recv().unwrap().is_none());
    producer.join().unwrap();
    instance.join().unwrap();
    assert_eq!(probe("chain-first").calls.load(Ordering::SeqCst), 2);
}

#[test]
fn timers_flush_while_a_producer_is_still_executing() {
    let mut value = graph("timer", 1);
    value["nodes"][0]["config"]["mode"] = json!("gate");
    value["execution"]["limits"]["max_pending_messages"] = json!(4);
    value["nodes"][1] =
        json!({"id":"consume", "kind":"builtin.batch", "config":{"max_items":5, "max_wait_ms":20}});
    value["edges"][1]["to_input"] = json!("item");
    value["outputs"][0]["port"] = json!("items");
    let instance = start(value, StreamOptions::default());
    let probe = probe("timer");
    let _release = Release(Arc::clone(&probe));
    instance.source.clone().send(json!(7)).unwrap();
    instance.source.close();
    wait_until(|| instance.summary().emitted_messages == 2);
    assert_eq!(
        instance.recv().unwrap().unwrap().outputs["value"],
        json!([7])
    );
    probe.release();
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}

#[test]
fn downstream_failure_wakes_a_producer_blocked_on_capacity() {
    let mut value = graph("failed-upstream", 1_000);
    value["nodes"][1] =
        json!({"id":"consume", "kind":"test.producer_sink", "config":{"key":"failed-sink"}});
    let instance = start(value, StreamOptions::default());
    let sink = probe("failed-sink");
    let _release = Release(Arc::clone(&sink));
    let producer = probe("failed-upstream");
    producer.dropped.store(0, Ordering::SeqCst);
    instance.source.clone().send(json!(0)).unwrap();
    wait_until(|| {
        producer.attempted.load(Ordering::SeqCst) == 5 && sink.calls.load(Ordering::SeqCst) == 1
    });
    sink.release();
    let error = instance.recv().unwrap_err().to_string();
    assert!(error.contains("downstream failure sentinel"), "{error}");
    assert!(instance.join().is_err());
    assert_eq!(producer.dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn producer_errors_and_panics_report_node_identity_and_preserve_delivered_outputs() {
    for mode in ["fail_after", "panic"] {
        let mut value = graph(mode, 1);
        value["nodes"][0]["config"]["mode"] = json!(mode);
        let instance = start(value, StreamOptions::default());
        let probe = probe(mode);
        let _release = Release(Arc::clone(&probe));
        instance.source.clone().send(json!(42)).unwrap();
        if mode == "fail_after" {
            assert_eq!(receive(&instance), 42);
            probe.release();
        }
        let error = instance.recv().unwrap_err();
        assert!(
            matches!(&error, mf_runtime::StreamError::Producer { definition_id, .. } if definition_id.as_str() == "produce"),
            "{error}"
        );
        assert!(error.to_string().contains("sentinel"), "{error}");
        assert!(instance.join().is_err());
    }
}

#[test]
fn ignored_invalid_sends_fail_publication_and_producer_observation_counts_outputs() {
    for mode in ["range", "invalid", "panic"] {
        let harness = capture::Harness::new(true);
        let mut value = graph(&format!("observed-{mode}"), 3);
        value["nodes"][0]["config"]["mode"] = json!(mode);
        let definition = serde_json::from_value::<WorkflowDefinition>(value.clone()).unwrap();
        let registry = NodeRegistry::from_inventory().unwrap();
        let plan = compile_definition(&definition, &registry).unwrap();
        let observation = plan
            .start_stream_observation(&harness.observer(), RunId::new())
            .unwrap();
        let instance = start(
            value,
            StreamOptions {
                observation: Some(observation),
                ..Default::default()
            },
        );
        instance.source.clone().send(json!(0)).unwrap();
        instance.source.close();
        if mode == "range" {
            for expected in 0..3 {
                assert_eq!(receive(&instance), expected);
            }
            assert!(instance.recv().unwrap().is_none());
            instance.join().unwrap();
        } else {
            assert!(instance.recv().is_err());
            assert!(instance.join().is_err());
        }
        let records: Vec<_> = harness
            .records()
            .iter()
            .filter(|record| record.scope == mf_telemetry::INSTRUMENTATION_SCOPE)
            .map(|record| StreamRecord::decode(record).unwrap())
            .collect();
        let finished = records.iter().find(|record| matches!(&record.payload, StreamPayload::Execution(Event::NodeFinished {node, ..}) if node.id == "produce")).unwrap();
        assert!(finished.identity.as_ref().unwrap().message.is_some());
        let spans = harness.spans.get_finished_spans().unwrap();
        let root = spans
            .iter()
            .find(|span| span.name == "mf.workflow")
            .unwrap();
        let producer_span = probe(&format!("observed-{mode}")).spans.lock().unwrap()[0];
        assert!(
            spans
                .iter()
                .any(|span| span.name == "mf.node" && span.span_context.span_id() == producer_span)
        );
        assert!(
            spans
                .iter()
                .filter(|span| span.name == "mf.node")
                .all(|span| span.parent_span_id == root.span_context.span_id())
        );
        if mode == "range" {
            assert_eq!(finished.emission_count.unwrap().get(), 3);
            assert!(matches!(
                finished.payload,
                StreamPayload::Execution(Event::NodeFinished {
                    outcome: Outcome::Succeeded,
                    ..
                })
            ));
        } else {
            let expected_phase = if mode == "invalid" {
                FailurePhase::Publication
            } else {
                FailurePhase::Execution
            };
            assert!(
                matches!(&finished.payload, StreamPayload::Execution(Event::NodeFinished { outcome: Outcome::Failed, failure: Some(failure), .. }) if failure.phase == expected_phase)
            );
        }
    }
}

#[test]
fn preparation_rejects_synchronous_and_mixed_domain_placement_without_execution() {
    let mut value = graph("preparation", 1);
    value.as_object_mut().unwrap().remove("execution");
    value["edges"].as_array_mut().unwrap().remove(0);
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("produce") && error.contains("synchronous"),
        "{error}"
    );
    let node = producer(json!({"key":"preparation"})).unwrap();
    assert!(matches!(
        Flow::new(
            vec![FlowNode::new("produce", node)],
            vec![],
            vec!["produce".into()],
            vec![]
        ),
        Err(mf_runtime::FlowBuildError::NonTaskNode { .. })
    ));
    let mut value = graph("preparation", 1);
    value["control_edges"] = json!([{
        "from_node":"feed", "from_output":"item", "to_node":"consume"
    }]);
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let error = compile_definition(&definition, &registry)
        .unwrap_err()
        .to_string();
    assert!(error.contains("domain"), "{error}");
    assert_eq!(probe("preparation").calls.load(Ordering::SeqCst), 0);
}

#[test]
fn producer_reads_declared_ancestor_context_on_its_worker() {
    let mut value = graph("context", 1);
    value["nodes"][0]["config"]["mode"] = json!("context");
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"copy", "kind":"builtin.identity"}));
    value["edges"][0]["to_node"] = json!("copy");
    value["edges"].as_array_mut().unwrap().push(json!({
        "from_node":"copy", "from_output":"value", "to_node":"produce", "to_input":"input"
    }));
    let instance = start(value, StreamOptions::default());
    instance.source.clone().send(json!(17)).unwrap();
    instance.source.close();
    assert_eq!(receive(&instance), 17);
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
}
