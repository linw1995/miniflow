extern crate mfn_core as _;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{
    ExecutionContext, Inputs, NodeExecutionError, NodeFactory, NodePorts, NodeRegistration,
    Outputs, PortSpec, PreparedNode, StreamNode, StreamOptions, ValueType, WorkflowArguments,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

type Gate = Arc<(Mutex<bool>, Condvar)>;
static GATES: OnceLock<Mutex<BTreeMap<String, Gate>>> = OnceLock::new();
static NEXT_GATE: AtomicUsize = AtomicUsize::new(0);

struct Source {
    gate: Option<Gate>,
    signal: bool,
    count: usize,
    check_future: bool,
}
impl StreamNode for Source {
    fn execute(
        &mut self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        if let Some(gate) = &self.gate {
            let mut ready = gate.0.lock().unwrap();
            if self.signal {
                *ready = true;
                gate.1.notify_all();
            } else {
                let (state, timeout) = gate
                    .1
                    .wait_timeout_while(ready, Duration::from_secs(5), |ready| !*ready)
                    .unwrap();
                if timeout.timed_out() && !*state {
                    return Err(NodeExecutionError::ExecutionFailed {
                        message: "independent producer did not start".into(),
                    });
                }
            }
        }
        if self.check_future {
            assert!(ctx.output("zz.value").is_err());
        }
        let start = inputs["start"].as_i64().unwrap();
        for item in 0..self.count {
            emitter
                .send(Outputs::from([("item".into(), json!(start + item as i64).into())]).into())?;
        }
        Ok(())
    }
}

inventory::submit! { NodeRegistration { kind: "test.autonomous_source", factory: NodeFactory::Plain(|config| {
    let gate = config.get("gate").and_then(Value::as_str).map(|key| {
        GATES.get_or_init(Mutex::default).lock().unwrap().entry(key.into()).or_insert_with(|| Arc::new((Mutex::new(false), Condvar::new()))).clone()
    });
    Ok(PreparedNode::stream(Source { gate, signal: config["signal"].as_bool().unwrap_or(false), count: config["count"].as_u64().unwrap_or(1) as usize, check_future: config["check_future"].as_bool().unwrap_or(false) }, NodePorts {
        inputs: vec![PortSpec::new("start", ValueType::Int64, true)],
        outputs: vec![PortSpec::new("item", ValueType::Int64, true)],
    }))
}) } }

fn prepare(mut value: Value) -> mf_runtime::PreparedStream {
    value["version"] = json!("2026-10-03");
    value["dependencies"] = json!({});
    if value.get("execution").is_none() {
        value["execution"] = json!({"mode":"stream"});
    }
    let definition = WorkflowDefinition::from_json(&value.to_string()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    instantiate_stream(&plan, &registry).unwrap()
}

#[test]
fn autonomous_source_uses_startup_arguments_and_drains_beyond_capacity() {
    let prepared = prepare(
        json!({"execution":{"mode":"stream", "limits":{"max_pending_messages":2,"workers":1}},
        "nodes":[{"id":"%input", "kind":"test.autonomous_source", "config":{"count":2000}}, {"id":"copy", "kind":"builtin.identity"}],
        "edges":[{"from_node":"%input", "from_output":"item", "to_node":"copy", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"copy", "port":"value"}]}),
    );
    let instance = prepared
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::try_from(json!({"%input":{"start":10}})).unwrap(),
            ..Default::default()
        })
        .unwrap();
    for expected in 10..2010 {
        assert_eq!(
            instance.recv().unwrap().unwrap().outputs["value"],
            json!(expected)
        );
    }
    assert!(instance.recv().unwrap().is_none());
    let summary = instance.join().unwrap();
    assert_eq!(summary.startup_frames, 1);
    assert_eq!(summary.emitted_messages, 2000);
    assert_eq!(summary.completed_frames, 2001);
    assert_eq!(summary.delivered_outputs, 2000);
}

#[test]
fn independent_startup_producers_progress_directly_and_after_tasks() {
    for after_tasks in [false, true] {
        let gate = NEXT_GATE.fetch_add(1, Ordering::SeqCst).to_string();
        let mut nodes = vec![
            json!({"id":"a", "kind":"test.autonomous_source", "config":{"gate":gate, "check_future":after_tasks}}),
            json!({"id":"zzz", "kind":"test.autonomous_source", "config":{"gate":gate, "signal":true}}),
        ];
        let mut edges = vec![];
        let arguments = if after_tasks {
            nodes.push(json!({"id":"0", "kind":"builtin.constant", "config":{"value":1}}));
            nodes.push(json!({"id":"zz", "kind":"builtin.constant", "config":{"value":2}}));
            edges.push(
                json!({"from_node":"0", "from_output":"value", "to_node":"a", "to_input":"start"}),
            );
            edges.push(json!({"from_node":"zz", "from_output":"value", "to_node":"zzz", "to_input":"start"}));
            WorkflowArguments::default()
        } else {
            WorkflowArguments::try_from(json!({"a":{"start":1}, "zzz":{"start":2}})).unwrap()
        };
        let prepared = prepare(
            json!({"execution":{"mode":"stream", "limits":{"max_pending_messages":3,"workers":1}},
            "nodes":nodes, "edges":edges, "outputs":[{"name":"value", "node":"a", "port":"item"}]}),
        );
        let instance = prepared
            .start_with_options(StreamOptions {
                arguments,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(instance.recv().unwrap().unwrap().outputs["value"], json!(1));
        assert!(instance.recv().unwrap().is_none());
        assert_eq!(instance.join().unwrap().emitted_messages, 2);
        GATES.get().unwrap().lock().unwrap().remove(&gate);
    }
}

#[test]
fn task_only_and_empty_streams_complete_without_a_sender() {
    for value in [
        json!({"nodes":[]}),
        json!({"nodes":[{"id":"copy", "kind":"builtin.identity"}], "outputs":[{"name":"value", "node":"copy", "port":"value"}]}),
    ] {
        let has_task = !value["nodes"].as_array().unwrap().is_empty();
        let prepared = prepare(value);
        let arguments = if has_task {
            WorkflowArguments::try_from(json!({"copy":{"input":42}})).unwrap()
        } else {
            WorkflowArguments::default()
        };
        let instance = prepared
            .start_with_options(StreamOptions {
                arguments,
                ..Default::default()
            })
            .unwrap();
        if has_task {
            assert_eq!(
                instance.recv().unwrap().unwrap().outputs["value"],
                json!(42)
            );
        }
        assert!(instance.recv().unwrap().is_none());
        let summary = instance.join().unwrap();
        assert_eq!(summary.completed_frames, 1);
        assert_eq!(summary.emitted_messages, 0);
    }
}

#[test]
fn explicit_channel_drains_on_close_and_cancellation_wakes_idle_sources() {
    let graph = json!({"nodes":[{"id":"feed", "kind":"builtin.channel", "config":{"item_type":"int"}}],
        "outputs":[{"name":"value", "node":"feed", "port":"item"}]});
    let mut prepared = prepare(graph.clone());
    let sender = prepared.channel("feed").unwrap();
    sender.send(json!(1)).unwrap();
    sender.send(json!(2)).unwrap();
    sender.close();
    let instance = prepared.start().unwrap();
    for value in [1, 2] {
        assert_eq!(
            instance.recv().unwrap().unwrap().outputs["value"],
            json!(value)
        );
    }
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().delivered_outputs, 2);
    assert!(matches!(
        sender.send(json!(3)),
        Err(mf_runtime::StreamError::Closed)
    ));

    let mut prepared = prepare(graph);
    let sender = prepared.channel("feed").unwrap();
    let instance = prepared.start().unwrap();
    drop(instance);
    assert!(sender.send(json!(4)).is_err());
}

#[test]
fn source_return_waits_for_final_output_acknowledgement() {
    let prepared = prepare(
        json!({"nodes":[{"id":"read", "kind":"test.autonomous_source"}],
        "outputs":[{"name":"value", "node":"read", "port":"item"}]}),
    );
    let instance = prepared
        .start_with_options(StreamOptions {
            arguments: WorkflowArguments::try_from(json!({"read":{"start":1}})).unwrap(),
            ..Default::default()
        })
        .unwrap();
    let delivery = instance.receive().unwrap().unwrap();
    assert_eq!(instance.summary().delivered_outputs, 0);
    delivery.acknowledge().unwrap();
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().delivered_outputs, 1);
}

#[test]
fn closing_one_source_does_not_close_another_source() {
    let mut prepared = prepare(json!({"nodes":[
        {"id":"a", "kind":"builtin.channel", "config":{"item_type":"int"}},
        {"id":"b", "kind":"builtin.channel", "config":{"item_type":"int"}}
    ], "outputs":[{"name":"value", "node":"a", "port":"item"}]}));
    let first = prepared.channel("a").unwrap();
    let second = prepared.channel("b").unwrap();
    let instance = prepared.start().unwrap();
    first.send(json!(1)).unwrap();
    first.close();
    assert_eq!(instance.recv().unwrap().unwrap().outputs["value"], json!(1));
    second.send(json!(2)).unwrap();
    second.close();
    assert!(instance.recv().unwrap().is_none());
    assert_eq!(instance.join().unwrap().emitted_messages, 2);
}
