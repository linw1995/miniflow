use mf_runtime::{
    EmissionSink, EventContext, EventEffects, EventNode, ExecutionContext, ExecutionDomain,
    ExecutionDomains, FlowNode, Inputs, MessageId, MessageSchedule, NodeEvent, NodeExecution,
    NodeExecutionError, NodeId, NodePorts, NodeResult, OutputVisibility, Outputs, PortSpec,
    PreparedNode, PreparedStream, PreparedSubgraph, StreamEmissionBinding, StreamError,
    StreamExecution, StreamInputResolver, StreamMessageProgram, StreamProgram, TaskNode, ValueType,
    WorkflowArguments, WorkflowRunError,
};
use snafu::ResultExt;
use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

type Check = Arc<dyn Fn(&ExecutionContext) + Send + Sync>;

fn outputs(value: i64) -> Outputs {
    Outputs::from([("value".into(), value.into())])
}

fn port() -> Vec<PortSpec> {
    vec![PortSpec::new("value", ValueType::Int64, true)]
}

struct Source {
    count: i64,
    check: Check,
    fail: bool,
}

impl EventNode for Source {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        if let Some(context) = context.input {
            (self.check)(context);
        }
        if matches!(event, NodeEvent::Timer) {
            return Ok(EventEffects::default());
        }
        if self.fail {
            return mf_runtime::NodeExecutionFailedSnafu {
                message: "recoverable Event failure",
            }
            .fail();
        }
        self.count += 1;
        Ok(EventEffects {
            emissions: vec![NodeResult::from(outputs(self.count)).into()],
            ..EventEffects::default()
        })
    }
}

struct Inspect {
    check: Check,
    fail: bool,
}
impl TaskNode for Inspect {
    fn execute(
        &self,
        _: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        (self.check)(context);
        if self.fail {
            return mf_runtime::NodeExecutionFailedSnafu {
                message: "recoverable message Task failure",
            }
            .fail();
        }
        let value = context
            .select_output("value", "source", "value", false)?
            .unwrap();
        Ok(Outputs::from([("value".into(), value)]).into())
    }
}

fn prepare_event(check: Check) -> PreparedStream {
    prepare_event_with_failures(check, false, false)
}

fn prepare_event_with_failures(
    check: Check,
    event_failure: bool,
    task_failure: bool,
) -> PreparedStream {
    let resolver: Arc<StreamInputResolver> = Arc::new(|_, _, _, _| Ok(Default::default()));
    PreparedStream::from_plan(
        StreamExecution::default(),
        vec![
            FlowNode::new(
                "source",
                PreparedNode::from_parts(
                    NodeExecution::Event(Box::new(Source {
                        count: 0,
                        check: check.clone(),
                        fail: event_failure,
                    })),
                    NodePorts {
                        inputs: vec![],
                        outputs: port(),
                    },
                ),
            ),
            FlowNode::new(
                "inspect",
                PreparedNode::from_parts(
                    NodeExecution::Task(Box::new(Inspect {
                        check,
                        fail: task_failure,
                    })),
                    NodePorts {
                        inputs: vec![],
                        outputs: port(),
                    },
                ),
            ),
        ],
        StreamProgram {
            execution_domains: ExecutionDomains::from_parts(
                (0..2)
                    .map(|id| ExecutionDomain {
                        id,
                        nodes: vec![NodeId::new(id)].into(),
                        positions: vec![id].into(),
                        predecessors: vec![].into(),
                        successors: vec![].into(),
                        first_position: id,
                        snapshot_visibility: OutputVisibility::Borrowed(&["source.value"]),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                vec![Cow::Owned(vec![]), Cow::Owned(vec![])].into(),
            ),
            messages: vec![
                StreamMessageProgram {
                    source: None,
                    schedule: MessageSchedule {
                        domains: vec![0].into(),
                        local: vec![0, usize::MAX].into(),
                        initial_deps: vec![0].into(),
                        successors: vec![Box::from([])].into(),
                    },
                    select_outputs: Some(Arc::new(|_| Ok(outputs(-1)))),
                },
                StreamMessageProgram {
                    source: Some(0),
                    schedule: MessageSchedule {
                        domains: vec![1].into(),
                        local: vec![usize::MAX, 0].into(),
                        initial_deps: vec![0].into(),
                        successors: vec![Box::from([])].into(),
                    },
                    select_outputs: Some(Arc::new(|context| {
                        Ok(Outputs::from([(
                            "value".into(),
                            context
                                .select_output("value", "inspect", "value", false)?
                                .unwrap(),
                        )]))
                    })),
                },
            ],
            emissions: vec![
                Some(StreamEmissionBinding {
                    message: 1,
                    connected: true,
                    terminal: false,
                    initialize: Arc::new(|plan, result| {
                        ExecutionContext::for_message(plan.emission_node(0), result)
                    }),
                }),
                None,
            ],
            inputs: vec![resolver.clone(), resolver],
        },
        Default::default(),
    )
}

#[test]
fn finite_invocation_separates_emissions_from_authoritative_terminal_outputs() {
    let body = PreparedSubgraph::streaming(vec![], port(), port(), |_, sink| {
        let mut skipped = NodeResult::from(outputs(1));
        skipped.skipped.insert("optional".into());
        sink.send(skipped).unwrap();
        sink.send(outputs(2).into()).unwrap();
        Ok(outputs(42))
    });
    let invocation = body.start_in_context(ExecutionContext::default()).unwrap();
    let first = invocation.recv().unwrap().unwrap();
    assert_eq!(first.result.outputs, outputs(1));
    assert!(first.result.skipped.contains("optional"));
    assert_eq!(first.message, None);
    // join must drain the rest, not deadlock behind the bounded output slot.
    assert_eq!(invocation.join().unwrap(), Some(outputs(42)));
    assert_eq!(
        PreparedSubgraph::new(vec![], port(), |_| Ok(outputs(7)))
            .start_in_context(ExecutionContext::default())
            .unwrap()
            .join()
            .unwrap(),
        Some(outputs(7))
    );
}

#[test]
fn event_factory_is_fresh_and_flushes_without_fabricating_terminal_results() {
    let count = Arc::new(AtomicUsize::new(0));
    let factories = count.clone();
    let body =
        PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), move || {
            factories.fetch_add(1, Ordering::SeqCst);
            Ok(prepare_event(Arc::new(|_| {})))
        });
    assert!(body.is_event_stream());
    assert!(body.outputs.is_empty());
    let invocations: Vec<_> = (0..3)
        .map(|_| body.start_in_context(ExecutionContext::default()).unwrap())
        .collect();
    for invocation in invocations {
        let mut seen = Vec::new();
        while let Some(emission) = invocation.recv().unwrap() {
            seen.push((
                emission.message.unwrap(),
                emission.result.outputs["value"].as_i64().unwrap(),
            ));
        }
        assert!(seen.contains(&(
            MessageId {
                domain: 0,
                sequence: 0
            },
            -1
        )));
        let events: Vec<_> = seen
            .into_iter()
            .filter(|(message, _)| message.domain == 1)
            .collect();
        assert_eq!(
            events,
            vec![
                (
                    MessageId {
                        domain: 1,
                        sequence: 0
                    },
                    1
                ),
                (
                    MessageId {
                        domain: 1,
                        sequence: 1
                    },
                    2
                )
            ]
        );
        assert_eq!(invocation.join().unwrap(), None);
    }
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn synchronous_event_adapter_drains_emissions_and_finite_bodies_stay_on_caller() {
    let caller = std::thread::current().id();
    let finite = PreparedSubgraph::streaming(vec![], port(), port(), move |_, sink| {
        assert_eq!(std::thread::current().id(), caller);
        sink.send(outputs(1).into()).unwrap();
        Ok(outputs(42))
    });
    struct Collect(Vec<NodeResult>);
    impl EmissionSink for Collect {
        fn send(&mut self, result: NodeResult) -> Result<(), StreamError> {
            self.0.push(result);
            Ok(())
        }
    }
    let mut sink = Collect(vec![]);
    assert_eq!(
        finite
            .execute_with_sink(&mut ExecutionContext::default(), &mut sink)
            .unwrap(),
        outputs(42)
    );
    let event = PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), || {
        Ok(prepare_event(Arc::new(|_| {})))
    });
    sink.0.clear();
    assert!(
        event
            .execute_with_sink(&mut ExecutionContext::default(), &mut sink)
            .unwrap()
            .is_empty()
    );
    assert_eq!(sink.0.len(), 3);
}

#[test]
fn finite_cancellation_wakes_blocked_publication_and_joins() {
    let (started, waiting) = mpsc::channel();
    let body = PreparedSubgraph::streaming(vec![], port(), port(), move |_, sink| {
        sink.send(outputs(1).into()).unwrap();
        started.send(()).unwrap();
        sink.send(outputs(2).into())
            .map_err(NodeExecutionError::from)
            .context(mf_runtime::NodeExecutionSnafu {
                definition_id: "body",
            })?;
        Ok(outputs(42))
    });
    let parent = ExecutionContext::default();
    let parent_cancellation = parent.cancellation();
    let invocation = body.start_in_context(parent).unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let cancellation = invocation.cancellation();
    cancellation.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "stop finite",
        }
        .build(),
    );
    assert!(
        invocation
            .recv()
            .unwrap_err()
            .to_string()
            .contains("stop finite")
    );
    assert!(
        invocation
            .join()
            .unwrap_err()
            .to_string()
            .contains("stop finite")
    );
    assert!(parent_cancellation.failure().is_none());
}

#[test]
fn event_cancellation_wakes_receivers_and_drop_releases_each_invocations_state() {
    let check: Check = Arc::new(|_| {});
    let factory_check = check.clone();
    let body =
        PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), move || {
            Ok(prepare_event(factory_check.clone()))
        });
    for explicitly_cancel in [true, false] {
        let context = ExecutionContext::default();
        let parent_cancellation = context.cancellation();
        let invocation = body.start_in_context(context).unwrap();
        let cancellation = invocation.cancellation();
        // Leave selections undrained: cancellation/drop must wake the coordinator
        // even when it is waiting for output acknowledgement.
        if explicitly_cancel {
            cancellation.cancel(
                mf_runtime::StreamExecutionSnafu {
                    message: "stop event",
                }
                .build(),
            );
            assert!(
                invocation
                    .recv()
                    .unwrap_err()
                    .to_string()
                    .contains("stop event")
            );
            assert!(
                invocation
                    .join()
                    .unwrap_err()
                    .to_string()
                    .contains("stop event")
            );
        } else {
            drop(invocation);
            assert!(cancellation.failure().is_some());
        }
        assert!(parent_cancellation.failure().is_none());
        assert_eq!(Arc::strong_count(&check), 2);
    }
}

#[test]
fn dropping_a_finite_invocation_cancels_blocked_publication_and_waits_for_cleanup() {
    struct Probe(mpsc::Sender<()>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.send(()).unwrap();
        }
    }
    let (started, waiting) = mpsc::channel();
    let (cleaned, cleanup) = mpsc::channel();
    let body = PreparedSubgraph::streaming(vec![], port(), port(), move |_, sink| {
        let _probe = Probe(cleaned.clone());
        sink.send(outputs(1).into()).unwrap();
        started.send(()).unwrap();
        sink.send(outputs(2).into())
            .map_err(NodeExecutionError::from)
            .context(mf_runtime::NodeExecutionSnafu {
                definition_id: "body",
            })?;
        Ok(outputs(42))
    });
    let context = ExecutionContext::default();
    let parent_cancellation = context.cancellation();
    let invocation = body.start_in_context(context).unwrap();
    let cancellation = invocation.cancellation();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(invocation);
    assert!(cancellation.failure().is_some());
    assert!(parent_cancellation.failure().is_none());
    cleanup.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn event_and_message_task_failures_do_not_cancel_the_receiving_execution() {
    use std::error::Error;
    for (event_failure, task_failure, expected) in [
        (true, false, "recoverable Event failure"),
        (false, true, "recoverable message Task failure"),
    ] {
        let body =
            PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), move || {
                Ok(prepare_event_with_failures(
                    Arc::new(|_| {}),
                    event_failure,
                    task_failure,
                ))
            });
        let mut parent = ExecutionContext::default();
        for asynchronous in [false, true] {
            let error = if asynchronous {
                body.start_in_context(parent.fork_domain_with_observation(None))
                    .unwrap()
                    .join()
                    .unwrap_err()
            } else {
                body.execute_in_context(&mut parent).unwrap_err()
            };
            let mut cause: &(dyn Error + 'static) = &error;
            let mut typed_failure = false;
            loop {
                let node_error = cause.downcast_ref::<NodeExecutionError>().or_else(|| {
                    cause
                        .downcast_ref::<Arc<NodeExecutionError>>()
                        .map(Arc::as_ref)
                });
                if matches!(node_error, Some(NodeExecutionError::ExecutionFailed { message }) if message == expected)
                {
                    typed_failure = true;
                }
                let Some(source) = cause.source() else {
                    break;
                };
                cause = source;
            }
            assert!(
                typed_failure,
                "ordinary failure lost its typed source: {error}"
            );
            assert!(parent.cancellation().failure().is_none());
        }
        assert_eq!(
            PreparedSubgraph::new(vec![], port(), |_| Ok(outputs(42)))
                .execute_in_context(&mut parent)
                .unwrap(),
            outputs(42)
        );
    }
}

#[test]
fn parent_cancellation_propagates_to_both_finite_and_event_invocations() {
    let finite = PreparedSubgraph::streaming(vec![], port(), port(), |_, sink| {
        for value in 0..3 {
            sink.send(outputs(value).into())
                .map_err(NodeExecutionError::from)
                .context(mf_runtime::NodeExecutionSnafu {
                    definition_id: "body",
                })?;
        }
        Ok(outputs(42))
    });
    let event = PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), || {
        Ok(prepare_event(Arc::new(|_| {})))
    });
    for body in [finite, event] {
        let parent = ExecutionContext::default();
        let cancellation = parent.cancellation();
        let invocation = body.start_in_context(parent).unwrap();
        assert!(invocation.recv().unwrap().is_some());
        cancellation.cancel(
            mf_runtime::StreamExecutionSnafu {
                message: "parent stopped",
            }
            .build(),
        );
        assert!(invocation.cancellation().failure().is_some());
        assert!(
            invocation
                .join()
                .unwrap_err()
                .to_string()
                .contains("parent stopped")
        );
    }
}

#[test]
fn direct_event_sink_delivery_failure_still_cancels_the_caller() {
    struct Reject;
    impl EmissionSink for Reject {
        fn send(&mut self, _: NodeResult) -> Result<(), StreamError> {
            mf_runtime::StreamExecutionSnafu {
                message: "downstream delivery failed",
            }
            .fail()
        }
    }
    let body = PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), || {
        Ok(prepare_event(Arc::new(|_| {})))
    });
    let mut parent = ExecutionContext::default();
    let error = body
        .execute_with_sink(&mut parent, &mut Reject)
        .unwrap_err();
    assert!(error.to_string().contains("downstream delivery failed"));
    assert!(
        matches!(parent.cancellation().failure(), Some(StreamError::Execution { message }) if message == "downstream delivery failed")
    );
}

#[test]
fn invocation_keeps_typed_body_errors_and_panics_are_joinable() {
    let failing = PreparedSubgraph::new(vec![], vec![], |_| {
        Err(NodeExecutionError::from(
            mf_runtime::StreamExecutionSnafu {
                message: "typed failure",
            }
            .build(),
        ))
        .context(mf_runtime::NodeExecutionSnafu {
            definition_id: "body",
        })
    });
    let invocation = failing
        .start_in_context(ExecutionContext::default())
        .unwrap();
    assert!(invocation.recv().unwrap().is_none());
    assert!(
        matches!(invocation.join().unwrap_err(), WorkflowRunError::NodeExecution { definition_id, .. } if definition_id.as_str() == "body")
    );
    let panicking = PreparedSubgraph::new(vec![], vec![], |_| panic!("body panic"));
    let invocation = panicking
        .start_in_context(ExecutionContext::default())
        .unwrap();
    assert!(invocation.recv().unwrap().is_none());
    assert!(
        invocation
            .join()
            .unwrap_err()
            .to_string()
            .contains("body panic")
    );
}

#[test]
fn event_frames_inherit_call_scope_worker_limit_and_snapshots() {
    let (sender, snapshots) = mpsc::channel();
    let recorder = mf_runtime::SnapshotRecorder::new(move |event| sender.send(event).unwrap());
    let mut parent = ExecutionContext::default();
    parent.set_snapshot_recorder(recorder);
    parent.configure_worker_limit(std::num::NonZeroUsize::new(1).unwrap());
    parent
        .run_call("outer", WorkflowArguments::default(), &[], |child| {
            let path = child.invocation_path();
            let expected = path.clone();
            let check: Check = Arc::new(move |context| {
                assert_eq!(context.invocation_path(), expected);
                assert_eq!(context.worker_limit().get(), 1);
                assert!(context.snapshot_recorder().is_some());
                assert!(context.worker_handle().is_some());
            });
            let invocation = prepare_event(check)
                .start_in_context(child.fork_domain_with_observation(child.observation().cloned()))
                .unwrap();
            while invocation.recv().unwrap().is_some() {}
            invocation.join().unwrap();
            let records: Vec<_> = snapshots.try_iter().collect();
            assert!(!records.is_empty());
            assert!(records.iter().all(|record| record.scope == path));
            Ok(())
        })
        .unwrap();
}

#[test]
fn step_observations_enter_captured_contexts_and_restore_the_receiver_context() {
    use mf_telemetry::{
        event::NodeIdentity,
        identity::{RunId, WorkflowId},
        observation::Observer,
    };
    use opentelemetry::Context;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Marker(&'static str);

    let observer = Observer::stderr(0);
    let mut run = observer
        .start(
            WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
            RunId::new(),
            vec![NodeIdentity {
                id: "outer".into(),
                kind: "fixture.outer".into(),
                path: vec![],
            }],
        )
        .unwrap();
    let (mut root_context, mut root_step) = {
        let _captured = Context::new().with_value(Marker("root")).attach();
        let mut context =
            ExecutionContext::default().fork_domain_with_observation(Some(run.clone()));
        let step = context.begin_step_observation("outer").unwrap();
        (context, step)
    };
    let (iteration, mut item, mut body_context, mut body_step) = {
        let _captured = Context::new().with_value(Marker("body")).attach();
        let iteration = run
            .iteration_observation(
                "outer",
                "outer-body",
                &[],
                vec![NodeIdentity {
                    id: "inner".into(),
                    kind: "fixture.inner".into(),
                    path: vec![],
                }],
            )
            .unwrap();
        let item = iteration.begin_item(0).unwrap();
        let mut context = ExecutionContext::for_body(Some(item.body_observation()));
        let step = context.begin_step_observation("inner").unwrap();
        (iteration, item, context, step)
    };
    let _receiver = Context::new().with_value(Marker("receiver")).attach();
    for (step, context, expected) in [
        (&mut root_step, &mut root_context, "root"),
        (&mut body_step, &mut body_context, "body"),
    ] {
        assert_eq!(
            Context::current().get::<Marker>(),
            Some(&Marker("receiver"))
        );
        {
            let _entered = step.enter();
            assert_eq!(Context::current().get::<Marker>(), Some(&Marker(expected)));
            step.started(context);
        }
        assert_eq!(
            Context::current().get::<Marker>(),
            Some(&Marker("receiver"))
        );
    }
    root_step.succeeded(&mut root_context, vec![], vec![], None);
    body_step.succeeded(&mut body_context, vec![], vec![], None);
    item.finish(None);
    drop(iteration);
    run.finish(None);
}

#[test]
fn top_level_messages_keep_fresh_budgets_while_nested_messages_share_the_caller_budget() {
    let exhaust_startup: Check = Arc::new(|context| {
        if context.current_node().is_none() {
            let node = FlowNode {
                definition_id: "budget-probe".into(),
                node: (),
                metadata: Default::default(),
                output_keys: Default::default(),
            };
            let mut budget = context.fork_domain_with_observation(None);
            // The startup Event already consumed one step. Charge the rest
            // without running thousands of unnecessary business task frames.
            for _ in 1..mf_runtime::MAX_SCHEDULED_STEPS {
                budget
                    .resolve_event_inputs(&node, |_, _| Ok(Default::default()))
                    .unwrap();
            }
        }
    });
    let top_level = prepare_event(exhaust_startup.clone()).start().unwrap();
    let mut values = Vec::new();
    while let Some(output) = top_level.recv().unwrap() {
        if output.message.domain == 1 {
            values.push(output.outputs["value"].as_i64().unwrap());
        }
    }
    assert_eq!(values, [1, 2]);
    top_level.join().unwrap();

    let nested = prepare_event(exhaust_startup)
        .start_in_context(ExecutionContext::default())
        .unwrap();
    while matches!(nested.recv(), Ok(Some(_))) {}
    assert!(
        nested
            .join()
            .unwrap_err()
            .to_string()
            .contains("scheduled-step budget")
    );
}

#[test]
fn concurrent_event_invocations_initialize_the_same_lazy_parent_pool() {
    let mut parent = ExecutionContext::default();
    parent.configure_worker_limit(std::num::NonZeroUsize::new(1).unwrap());
    let owner = parent.worker_scope();
    assert!(owner.handle().is_none());
    let instances: Vec<_> = (0..3)
        .map(|_| {
            prepare_event(Arc::new(|context| {
                assert_eq!(context.worker_handle().unwrap().worker_count(), 1);
            }))
            .start_in_context(parent.fork_domain_with_observation(None))
            .unwrap()
        })
        .collect();
    let handle = owner
        .handle()
        .expect("nested streams initialize the shared lazy pool");
    for instance in instances {
        while instance.recv().unwrap().is_some() {}
        instance.join().unwrap();
        assert!(handle.is_active());
    }
    drop(owner);
    assert!(!handle.is_active());
}

#[test]
fn nested_invocations_reuse_and_help_a_single_parent_worker() {
    let pool = mf_runtime::WorkerPool::new(1, mf_runtime::WorkerJob::run).unwrap();
    let mut context = ExecutionContext::default();
    context.set_worker_handle(pool.handle());
    context.configure_worker_limit(std::num::NonZeroUsize::new(1).unwrap());
    let result = pool.handle().run_parallel(vec![move || {
        let caller = std::thread::current().id();
        let check: Check = Arc::new(move |context| {
            if context.current_node() == Some("inspect") {
                assert_eq!(std::thread::current().id(), caller);
            }
            assert_eq!(context.worker_handle().unwrap().worker_count(), 1);
            assert_eq!(context.worker_limit().get(), 1);
        });
        let source_check = check.clone();
        let event =
            PreparedSubgraph::from_stream_factory(vec![], Default::default(), port(), move || {
                Ok(prepare_event(source_check.clone()))
            });
        assert!(event.execute_in_context(&mut context).unwrap().is_empty());

        let finite = PreparedSubgraph::new(vec![], port(), |context| {
            let values = context
                .worker_handle()
                .unwrap()
                .run_parallel((0..2).map(|value| move || value).collect());
            Ok(outputs(values.into_iter().sum()))
        });
        let invocation = finite.start_in_context(context).unwrap();
        invocation.join().unwrap()
    }]);
    assert_eq!(result, vec![Some(outputs(1))]);
}

#[cfg(feature = "tokio")]
#[test]
fn event_startup_and_message_tasks_reuse_the_parent_future_backend() {
    let parent = ExecutionContext::default();
    let backend = parent
        .execute_future(
            async { tokio::runtime::Handle::current().id().to_string() },
            None,
        )
        .unwrap();
    let check: Check = Arc::new(move |context| {
        let actual = context
            .execute_future(
                async { tokio::runtime::Handle::current().id().to_string() },
                None,
            )
            .unwrap();
        assert_eq!(actual, backend);
    });
    let instance = prepare_event(check).start_in_context(parent).unwrap();
    while instance.recv().unwrap().is_some() {}
    instance.join().unwrap();
}
