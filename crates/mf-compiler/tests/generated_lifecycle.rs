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
