use mf_runtime::{
    DiscardSink, EmissionSink, Emitter, ExecutionContext, Inputs, NodeBuildError, NodeExecution,
    NodeExecutionError, NodeMetadata, NodeResult, NodeValue, PreparedNode, StreamCancellation,
    StreamError, StreamInvocation, TypedEmitter, TypedNodeResult, TypedTaskStreamNode,
};
use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(NodeValue)]
struct Input {
    count: usize,
}

#[derive(NodeValue)]
struct Output {
    text: String,
}

#[derive(NodeValue)]
struct Delta {
    delta: f64,
}

struct Combined {
    calls: Arc<AtomicUsize>,
    returned: Arc<AtomicBool>,
    emit: bool,
    invalid: bool,
    ignore_error: bool,
}

impl TypedTaskStreamNode for Combined {
    type Input = Input;
    type Output = Output;
    type Emission = Delta;

    fn execute(
        &self,
        input: Input,
        _: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, Delta>,
    ) -> Result<TypedNodeResult<Output>, NodeExecutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.emit {
            for index in 0..input.count {
                let result = emitter.send(Delta {
                    delta: if self.invalid { f64::NAN } else { index as f64 },
                });
                if !self.ignore_error {
                    result?;
                }
            }
        }
        self.returned.store(true, Ordering::SeqCst);
        Ok(Output {
            text: format!("processed {}", input.count),
        }
        .into())
    }
}

fn combined(emit: bool, invalid: bool, ignore_error: bool) -> Combined {
    Combined {
        calls: Arc::default(),
        returned: Arc::default(),
        emit,
        invalid,
        ignore_error,
    }
}

fn inputs(count: usize) -> Inputs {
    Input { count }.into_values().unwrap()
}

#[derive(Default)]
struct Collect {
    values: Vec<NodeResult>,
    returned: Arc<AtomicBool>,
    fail: bool,
}

impl EmissionSink for Collect {
    fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
        assert!(!self.returned.load(Ordering::SeqCst));
        if self.fail {
            return Err(StreamError::Execution {
                message: "receiver disconnected".into(),
            });
        }
        self.values.push(result);
        Ok(())
    }
}

fn has_source<T: Error + 'static>(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.is::<T>() {
            return true;
        }
        current = error.source();
    }
    false
}

#[test]
fn preparation_reflects_both_contracts_without_invoking_business_logic() {
    let node = combined(true, false, false);
    let calls = node.calls.clone();
    let prepared = PreparedNode::typed_task_stream(node, NodeMetadata::default()).unwrap();
    assert_eq!(prepared.metadata.ports.inputs, Input::ports());
    assert_eq!(prepared.metadata.ports.outputs, Output::ports());
    assert_eq!(prepared.metadata.stream_outputs, Delta::ports());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(prepared.execution.as_task_node().is_some());
}

#[test]
fn one_invocation_delivers_incremental_values_before_its_final_result() {
    let node = combined(true, false, false);
    let calls = node.calls.clone();
    let mut sink = Collect {
        returned: node.returned.clone(),
        ..Default::default()
    };
    let prepared = PreparedNode::typed_task_stream(node, NodeMetadata::default()).unwrap();
    let NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected combined executor")
    };
    let result = executor
        .execute_with_sink(inputs(3), &mut ExecutionContext::default(), &mut sink)
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.outputs["text"].as_str(), Some("processed 3"));
    assert_eq!(sink.values.len(), 3);
    for (index, result) in sink.values.iter().enumerate() {
        assert_eq!(result.outputs["delta"].as_f64(), Some(index as f64));
        assert!(!result.outputs.contains_key("text"));
    }
}

#[test]
fn zero_emissions_do_not_suppress_the_terminal_result() {
    let node = combined(false, false, false);
    let mut sink = Collect::default();
    let prepared = PreparedNode::typed_task_stream(node, NodeMetadata::default()).unwrap();
    let NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected combined executor")
    };
    let result = executor
        .execute_with_sink(inputs(7), &mut ExecutionContext::default(), &mut sink)
        .unwrap();
    assert_eq!(result.outputs["text"].as_str(), Some("processed 7"));
    assert!(sink.values.is_empty());
}

#[test]
fn terminal_task_adaptation_discards_without_an_unread_receiver() {
    let node = combined(true, false, false);
    let calls = node.calls.clone();
    let prepared = PreparedNode::typed_task_stream(node, NodeMetadata::default()).unwrap();
    let task = prepared.execution.into_task_node().unwrap();
    for count in [100, 200] {
        let result = task
            .execute(inputs(count), &mut ExecutionContext::default())
            .unwrap();
        assert_eq!(
            result.outputs["text"].as_str(),
            Some(format!("processed {count}").as_str())
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn ignored_encoding_failure_remains_failure_when_output_is_discarded() {
    let prepared =
        PreparedNode::typed_task_stream(combined(true, true, true), NodeMetadata::default())
            .unwrap();
    let error = prepared
        .execution
        .into_task_node()
        .unwrap()
        .execute(inputs(2), &mut ExecutionContext::default())
        .unwrap_err();
    assert!(has_source::<Box<mf_runtime::OutputEncodeError>>(&error));
    assert!(error.to_string().contains("stream output"));
}

#[test]
fn ignored_delivery_failure_suppresses_terminal_success() {
    let prepared =
        PreparedNode::typed_task_stream(combined(true, false, true), NodeMetadata::default())
            .unwrap();
    let NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected combined executor")
    };
    let mut sink = Collect {
        fail: true,
        ..Default::default()
    };
    let error = executor
        .execute_with_sink(inputs(2), &mut ExecutionContext::default(), &mut sink)
        .unwrap_err();
    assert!(has_source::<StreamError>(&error));
    assert!(error.to_string().contains("receiver disconnected"));
    assert!(sink.values.is_empty());
}

#[test]
fn discarded_dynamic_emissions_still_validate_types_and_required_fields() {
    let mut sink = DiscardSink;
    let mut emitter = Emitter::new(
        "source",
        &Delta::ports(),
        StreamCancellation::default(),
        &mut sink,
    );
    assert!(!emitter.is_connected());
    let error = emitter.send(NodeResult::default()).unwrap_err();
    assert!(error.to_string().contains("required port `delta`"));
    assert!(emitter.finish().is_err());

    let mut emitter = Emitter::new(
        "source",
        &Delta::ports(),
        StreamCancellation::default(),
        &mut sink,
    );
    let error = emitter
        .send(NodeResult {
            outputs: [("delta".into(), "wrong type".into())].into(),
            ..Default::default()
        })
        .unwrap_err();
    assert!(has_source::<mf_runtime::TypeMismatch>(&error));
    assert!(error.to_string().contains("source"));
}

#[test]
fn competing_stream_declarations_fail_during_preparation() {
    let error = PreparedNode::typed_task_stream(
        combined(false, false, false),
        NodeMetadata {
            stream_outputs: Delta::ports(),
            ..Default::default()
        },
    )
    .err()
    .unwrap();
    assert!(matches!(
        error,
        NodeBuildError::ConflictingStreamDeclarations
    ));
}

#[test]
fn cancellation_is_checked_even_without_emissions() {
    let node = combined(false, false, false);
    let calls = node.calls.clone();
    let prepared = PreparedNode::typed_task_stream(node, NodeMetadata::default()).unwrap();
    let mut context = ExecutionContext::default();
    context.cancellation().cancel(StreamError::Execution {
        message: "cancelled".into(),
    });
    assert!(
        prepared
            .execution
            .into_task_node()
            .unwrap()
            .execute(inputs(1), &mut context)
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn invocation_identities_do_not_reuse_a_previous_call() {
    let context = ExecutionContext::default();
    let first = StreamInvocation::new(&context).unwrap();
    let second = StreamInvocation::new(&context).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(first.node, second.node);
    assert_eq!(first.path, second.path);
}

struct Dynamic {
    ports: Vec<mf_runtime::PortSpec>,
}

impl mf_runtime::NodePortContract for Dynamic {
    fn ports(&self) -> mf_runtime::NodePorts {
        mf_runtime::NodePorts::from_types::<Input, Output>()
    }

    fn stream_outputs(&self) -> Vec<mf_runtime::PortSpec> {
        self.ports.clone()
    }
}

impl mf_runtime::TaskStreamNode for Dynamic {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
        _: &mut Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        panic!("preparation must not execute the provider")
    }
}

#[test]
fn dynamic_contracts_reject_colliding_and_duplicate_stream_names() {
    let collision = PreparedNode::task_stream(
        Dynamic {
            ports: Output::ports(),
        },
        NodeMetadata::default(),
    )
    .err()
    .unwrap();
    assert!(matches!(
        collision,
        NodeBuildError::OutputCategoryCollision { .. }
    ));
    let duplicate = PreparedNode::task_stream(
        Dynamic {
            ports: [Delta::ports(), Delta::ports()].concat(),
        },
        NodeMetadata::default(),
    )
    .err()
    .unwrap();
    assert!(matches!(
        duplicate,
        NodeBuildError::DuplicateStreamOutput { .. }
    ));
    let prepared = PreparedNode::task_stream(
        Dynamic {
            ports: Delta::ports(),
        },
        NodeMetadata::default(),
    )
    .unwrap();
    assert_eq!(
        prepared.execution.terminal_outputs(&prepared.metadata),
        Output::ports()
    );
    assert_eq!(
        prepared.execution.stream_outputs(&prepared.metadata),
        Delta::ports()
    );
}

#[test]
fn invocation_paths_survive_body_and_domain_context_forks() {
    let mut context = ExecutionContext::default();
    let scope = mf_runtime::ExecutionScope::new(
        "repeat",
        "%loop",
        3,
        Default::default(),
        Default::default(),
    )
    .unwrap();
    context
        .run_scope(scope, |context| {
            let child = context.fork_body(None).fork_domain_with_observation(None);
            let invocation = StreamInvocation::new(&child).unwrap();
            assert_eq!(invocation.path.len(), 1);
            assert_eq!(invocation.path[0].loop_id, "repeat");
            assert_eq!(invocation.path[0].index.get(), 3);
            Ok(())
        })
        .unwrap();
}

#[derive(NodeValue)]
struct SensitiveDelta {
    delta: mf_runtime::ValueRef,
}

struct Sensitive;

impl TypedTaskStreamNode for Sensitive {
    type Input = Input;
    type Output = Output;
    type Emission = SensitiveDelta;

    fn execute(
        &self,
        _: Input,
        _: &mut ExecutionContext,
        emitter: &mut TypedEmitter<'_, '_, SensitiveDelta>,
    ) -> Result<TypedNodeResult<Output>, NodeExecutionError> {
        emitter.send(SensitiveDelta {
            delta: mf_runtime::ValueRef::from("credential").into_secret(),
        })?;
        Ok(Output {
            text: "complete".into(),
        }
        .into())
    }
}

#[test]
fn combined_emission_snapshots_are_redacted_independently_of_business_delivery() {
    let prepared = PreparedNode::typed_task_stream(Sensitive, NodeMetadata::default()).unwrap();
    let NodeExecution::TaskStream(executor) = prepared.execution else {
        panic!("expected combined executor")
    };
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = events.clone();
    let mut context = ExecutionContext::default();
    context.set_snapshot_recorder(mf_runtime::SnapshotRecorder::new(move |event| {
        captured.lock().unwrap().push(event)
    }));
    let mut sink = Collect::default();
    executor
        .execute_with_sink(inputs(1), &mut context, &mut sink)
        .unwrap();
    assert_eq!(sink.values[0].outputs["delta"].as_str(), Some("credential"));
    assert_eq!(
        events.lock().unwrap()[0].snapshot.outputs["delta"].as_str(),
        Some("[REDACTED]")
    );
    assert!(matches!(
        events.lock().unwrap()[0].snapshot.outcome,
        mf_runtime::SnapshotOutcome::Emitted
    ));
}

#[test]
fn publication_deadlines_interrupt_bounded_delivery_without_a_timer_thread() {
    struct QueueSink(mf_runtime::LivePublisher);
    impl EmissionSink for QueueSink {
        fn send(&mut self, _: NodeResult) -> Result<(), StreamError> {
            unreachable!()
        }
        fn send_with_cancellation(
            &mut self,
            result: NodeResult,
            cancellation: &StreamCancellation,
        ) -> Result<(), StreamError> {
            self.0.publish(
                mf_runtime::LiveOutputEvent {
                    export: "answer".into(),
                    source: vec!["llm".into()],
                    invocation: StreamInvocation {
                        id: 0,
                        node: "llm".into(),
                        path: vec![],
                    },
                    event: mf_runtime::LiveOutputKind::Value {
                        value: result.outputs["delta"].clone(),
                    },
                },
                cancellation,
            )
        }
    }
    let (publisher, _receiver) = mf_runtime::live_channel(mf_runtime::LiveLimits {
        max_pending_records: 1,
    })
    .unwrap();
    let mut sink = QueueSink(publisher);
    let cancellation = StreamCancellation::default();
    let mut emitter = Emitter::new("llm", &Delta::ports(), cancellation.clone(), &mut sink);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(40);
    let result = || Delta { delta: 1.0 }.into_values().unwrap().into();
    emitter.send_with_deadline(result(), deadline).unwrap();
    let error = emitter.send_with_deadline(result(), deadline).unwrap_err();
    assert!(error.to_string().contains("publication deadline exceeded"));
    assert!(emitter.finish().is_err());
    assert!(cancellation.failure().is_none());
    let (publisher, receiver) = mf_runtime::live_channel(Default::default()).unwrap();
    let mut sink = QueueSink(publisher);
    let mut emitter = Emitter::new("llm", &Delta::ports(), cancellation, &mut sink);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(20);
    emitter.send_with_deadline(result(), deadline).unwrap();
    receiver.recv().unwrap().unwrap();
    std::thread::sleep(
        deadline.saturating_duration_since(std::time::Instant::now())
            + std::time::Duration::from_millis(2),
    );
    emitter.finish().unwrap();
    drop(emitter);
    drop(sink);
    assert!(receiver.recv().unwrap().is_none());
}
