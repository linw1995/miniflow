use mf_compiler::build_flow;
use mf_runtime::{
    ExecutionContext, FlowNode, GeneratedDomainExecutor, GeneratedNodeResult, NodePorts, NodeValue,
    TypedConstructor, TypedNodeResult, TypedNodeValue, TypedTaskHandle, TypedTaskNode,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(NodeValue)]
#[value(typed)]
struct Empty {}

#[derive(NodeValue)]
#[value(typed)]
struct Produced {
    value: String,
    unused: Vec<f64>,
}

#[derive(NodeValue)]
#[value(typed)]
struct Text {
    value: String,
}

struct Source {
    invalid: bool,
    calls: Arc<AtomicUsize>,
    pointer: Arc<AtomicUsize>,
}
impl TypedTaskNode for Source {
    type Input = Empty;
    type Output = Produced;
    fn execute(
        &self,
        _: Empty,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Produced>, mf_runtime::NodeExecutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let value = "owned payload".to_owned();
        self.pointer
            .store(value.as_ptr() as usize, Ordering::SeqCst);
        Ok(Produced {
            value,
            unused: vec![if self.invalid { f64::NAN } else { 1.0 }],
        }
        .into())
    }
}

struct Sink {
    calls: Arc<AtomicUsize>,
    pointer: Arc<AtomicUsize>,
}
impl TypedTaskNode for Sink {
    type Input = Text;
    type Output = Text;
    fn execute(
        &self,
        input: Text,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Text>, mf_runtime::NodeExecutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.pointer
            .store(input.value.as_ptr() as usize, Ordering::SeqCst);
        Ok(input.into())
    }
}

struct Run {
    flow: mf_runtime::Flow,
    source_calls: Arc<AtomicUsize>,
    sink_calls: Arc<AtomicUsize>,
    source_pointer: Arc<AtomicUsize>,
    sink_pointer: Arc<AtomicUsize>,
}

fn flow(invalid: bool) -> Run {
    let source_calls = Arc::new(AtomicUsize::new(0));
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let source_pointer = Arc::new(AtomicUsize::new(0));
    let sink_pointer = Arc::new(AtomicUsize::new(0));
    let source = TypedTaskHandle::new(
        Source {
            invalid,
            calls: source_calls.clone(),
            pointer: source_pointer.clone(),
        },
        NodePorts::default(),
        TypedConstructor {
            package: "fixture",
            path: &["source"],
        },
        true,
    )
    .unwrap();
    let sink = TypedTaskHandle::new(
        Sink {
            calls: sink_calls.clone(),
            pointer: sink_pointer.clone(),
        },
        NodePorts::default(),
        TypedConstructor {
            package: "fixture",
            path: &["sink"],
        },
        true,
    )
    .unwrap();
    let flow = build_flow(vec![FlowNode::new("source", source.prepared()), FlowNode::new("sink", sink.prepared())],
        serde_json::from_value(json!([{"from_node":"source", "from_output":"value", "to_node":"sink", "to_input":"value"}])).unwrap(),
        vec!["source".into(), "sink".into()],
        serde_json::from_value(json!([{"name":"result", "node":"sink", "port":"value"}])).unwrap()).unwrap();
    let executor: Arc<GeneratedDomainExecutor> = Arc::new(move |flow, ctx| {
        let mut fields = None;
        flow.execute_generated_position(0, &[], ctx, |inputs, ctx| {
            let result = source.execute(source.decode_inputs(inputs)?, ctx)?;
            let publication = GeneratedNodeResult::typed(&result)?;
            fields = Some(result.outputs.into_fields());
            Ok(publication)
        })?;
        flow.execute_generated_position(1, &["value"], ctx, |_, ctx| {
            let (value, _unused) = fields.take().expect("validated producer presence");
            GeneratedNodeResult::encoded(sink.execute(sink.input_from_fields((value,)), ctx)?)
        })
    });
    Run {
        flow: flow.with_generated_domains([(0, executor)]),
        source_calls,
        sink_calls,
        source_pointer,
        sink_pointer,
    }
}

#[test]
fn direct_transfer_preserves_owned_allocation_and_repeated_invocations() {
    let run = flow(false);
    for _ in 0..2 {
        assert_eq!(
            run.flow.execute().unwrap()["result"],
            json!("owned payload")
        );
        assert_eq!(
            run.source_pointer.load(Ordering::SeqCst),
            run.sink_pointer.load(Ordering::SeqCst)
        );
    }
    assert_eq!(run.source_calls.load(Ordering::SeqCst), 2);
    assert_eq!(run.sink_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn invalid_unused_outputs_fail_before_a_successor_and_match_dynamic_errors() {
    let run = flow(true);
    let typed = run.flow.execute().unwrap_err();
    let recorder = mf_runtime::SnapshotRecorder::memory();
    let mut ctx = ExecutionContext::default();
    ctx.set_snapshot_recorder(recorder);
    let dynamic = run.flow.execute_in_context(&mut ctx).unwrap_err();
    assert_eq!(typed.to_string(), dynamic.to_string());
    assert!(typed.to_string().contains("/unused/0"));
    assert_eq!(run.source_calls.load(Ordering::SeqCst), 2);
    assert_eq!(run.sink_calls.load(Ordering::SeqCst), 0);
    assert!(ctx.output("source.value").is_err());
}

#[test]
fn snapshot_fallback_keeps_intermediate_values_and_the_same_instances() {
    let run = flow(false);
    run.flow.execute().unwrap();
    let recorder = mf_runtime::SnapshotRecorder::memory();
    let mut ctx = ExecutionContext::default();
    ctx.set_snapshot_recorder(recorder.clone());
    assert_eq!(
        run.flow.execute_in_context(&mut ctx).unwrap()["result"],
        json!("owned payload")
    );
    assert!(ctx.output("source.value").is_ok());
    assert_eq!(run.source_calls.load(Ordering::SeqCst), 2);
    assert_eq!(run.sink_calls.load(Ordering::SeqCst), 2);
    assert!(!recorder.history().is_empty());
}

#[test]
fn independent_typed_domains_use_private_frames_and_the_common_worker_limit() {
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut nodes = Vec::new();
    let mut bindings = Vec::new();
    let mut pointers = Vec::new();
    for domain in 0..2 {
        let source_pointer = Arc::new(AtomicUsize::new(0));
        let sink_pointer = Arc::new(AtomicUsize::new(0));
        let source = TypedTaskHandle::new(
            Source {
                invalid: false,
                calls: Arc::new(AtomicUsize::new(0)),
                pointer: source_pointer.clone(),
            },
            NodePorts::default(),
            TypedConstructor {
                package: "fixture",
                path: &["source"],
            },
            true,
        )
        .unwrap();
        let sink = TypedTaskHandle::new(
            Sink {
                calls: Arc::new(AtomicUsize::new(0)),
                pointer: sink_pointer.clone(),
            },
            NodePorts::default(),
            TypedConstructor {
                package: "fixture",
                path: &["sink"],
            },
            true,
        )
        .unwrap();
        nodes.push(FlowNode::new(format!("source_{domain}"), source.prepared()));
        nodes.push(FlowNode::new(format!("sink_{domain}"), sink.prepared()));
        let barrier = barrier.clone();
        let executor: Arc<GeneratedDomainExecutor> = Arc::new(move |flow, ctx| {
            barrier.wait();
            let mut fields = None;
            flow.execute_generated_position(domain * 2, &[], ctx, |inputs, ctx| {
                let result = source.execute(source.decode_inputs(inputs)?, ctx)?;
                let publication = GeneratedNodeResult::typed(&result)?;
                fields = Some(result.outputs.into_fields());
                Ok(publication)
            })?;
            flow.execute_generated_position(domain * 2 + 1, &["value"], ctx, |_, ctx| {
                let (value, _unused) = fields.take().expect("local producer presence");
                GeneratedNodeResult::encoded(sink.execute(sink.input_from_fields((value,)), ctx)?)
            })
        });
        bindings.push((domain, executor));
        pointers.push((source_pointer, sink_pointer));
    }
    let flow = build_flow(nodes,
        serde_json::from_value(json!([
            {"from_node":"source_0", "from_output":"value", "to_node":"sink_0", "to_input":"value"},
            {"from_node":"source_1", "from_output":"value", "to_node":"sink_1", "to_input":"value"}
        ])).unwrap(),
        vec!["source_0".into(), "sink_0".into(), "source_1".into(), "sink_1".into()],
        serde_json::from_value(json!([
            {"name":"left", "node":"sink_0", "port":"value"}, {"name":"right", "node":"sink_1", "port":"value"}
        ])).unwrap()).unwrap().with_generated_domains(bindings);
    let outputs = flow
        .execute_with_options(mf_runtime::RuntimeOptions {
            max_parallel_domains: std::num::NonZeroUsize::new(2).unwrap(),
        })
        .unwrap();
    assert_eq!(outputs["left"], json!("owned payload"));
    assert_eq!(outputs["right"], json!("owned payload"));
    for (source, sink) in pointers {
        assert_eq!(source.load(Ordering::SeqCst), sink.load(Ordering::SeqCst));
    }
}

#[test]
fn generated_calls_preserve_missing_before_skip_without_input_assembly() {
    struct Gate;
    impl mf_runtime::TaskNode for Gate {
        fn execute(
            &self,
            _: mf_runtime::Inputs,
            _: &mut ExecutionContext,
        ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
            Ok(mf_runtime::NodeResult {
                skipped: std::collections::BTreeSet::from(["trigger".into()]),
                ..Default::default()
            })
        }
    }
    for missing in [false, true] {
        let mut ctx = ExecutionContext::default();
        let gate = FlowNode::new(
            "gate",
            mf_runtime::PreparedNode::new(
                Gate,
                NodePorts {
                    inputs: vec![],
                    outputs: vec![mf_runtime::PortSpec::new(
                        "trigger",
                        mf_runtime::ValueType::Any,
                        false,
                    )],
                },
            ),
        )
        .into_task()
        .unwrap();
        mf_runtime::execute_node_in_context(&gate, &[], &mut ctx).unwrap();
        let run = flow(false);
        let mut dependencies = vec![mf_runtime::ExecutionDependency {
            input: None,
            source_node: "gate",
            source_output: "trigger",
        }];
        if missing {
            dependencies.push(mf_runtime::ExecutionDependency {
                input: None,
                source_node: "missing",
                source_output: "value",
            });
        }
        let result = mf_runtime::execute_generated_node_in_context(
            &run.flow.nodes()[0],
            dependencies,
            &mut ctx,
            &[],
            |_, _| panic!("skipped or missing dependencies must prevent typed input assembly"),
        );
        if missing {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("missing.value"));
            assert!(std::error::Error::source(&error).is_some());
        } else {
            result.unwrap();
            assert_eq!(
                ctx.output("source.value").unwrap(),
                mf_runtime::ContextValue::Skipped
            );
        }
        assert_eq!(run.source_calls.load(Ordering::SeqCst), 0);
    }
}
