use mf_runtime::*;
use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

type Invoke =
    dyn Fn(Inputs, &mut ExecutionContext) -> Result<NodeResult, NodeExecutionError> + Send + Sync;
struct FunctionTask(Box<Invoke>);
impl TaskNode for FunctionTask {
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        (self.0)(inputs, ctx)
    }
}

fn port(name: &'static str) -> PortSpec {
    PortSpec::new(name, ValueType::String, true)
}
fn node(
    id: &str,
    inputs: Vec<PortSpec>,
    outputs: Vec<PortSpec>,
    run: impl Fn(Inputs, &mut ExecutionContext) -> Result<NodeResult, NodeExecutionError>
    + Send
    + Sync
    + 'static,
) -> TaskFlowNode {
    FlowNode {
        definition_id: id.into(),
        node: Box::new(FunctionTask(Box::new(run))),
        metadata: NodeMetadata::new(NodePorts { inputs, outputs }),
        output_keys: Default::default(),
    }
}
fn values(name: &str, value: &str) -> Outputs {
    Outputs::from([(name.into(), value.into())])
}
fn binding(caller: usize, target: usize, name: &str, target_id: &str) -> PreparedCall {
    PreparedCall {
        target: NodeIdentity {
            id: target_id.into(),
            kind: "test.tool".into(),
            path: vec![],
        },
        binding: ResolvedCallBinding {
            id: CallBindingId::new(0),
            name: name.into(),
            caller: NodeId::new(caller),
            target: NodeId::new(target),
            inputs: vec![],
            dependencies: vec![],
            success_dependencies: vec![],
            prerequisites: vec![],
        },
        visibility: OutputVisibility::Borrowed(&[]),
        guard: Arc::new(|_| Ok(BTreeSet::new())),
    }
}
fn flow(nodes: Vec<TaskFlowNode>, scheduled: &[usize], sequential: bool) -> Flow {
    let domains = scheduled
        .iter()
        .enumerate()
        .map(|(id, &position)| ExecutionDomain {
            id,
            nodes: vec![NodeId::new(position)].into(),
            positions: vec![position].into(),
            predecessors: if sequential && id > 0 {
                vec![id - 1]
            } else {
                vec![]
            }
            .into(),
            successors: if sequential && id + 1 < scheduled.len() {
                vec![id + 1]
            } else {
                vec![]
            }
            .into(),
            first_position: position,
            snapshot_visibility: OutputVisibility::from_sorted(
                scheduled[..id]
                    .iter()
                    .flat_map(|&index| nodes[index].output_keys().iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            ),
        })
        .collect::<Vec<_>>();
    let ancestors = (0..domains.len())
        .map(|id| {
            if sequential {
                (0..id).collect::<Vec<_>>()
            } else {
                vec![]
            }
            .into()
        })
        .collect::<Vec<_>>();
    let len = nodes.len();
    Flow::from_plan(
        nodes,
        FlowPlan {
            connections: vec![].into(),
            dependencies: vec![vec![].into(); len].into(),
            execution_order: (0..len).map(NodeId::new).collect::<Vec<_>>().into(),
            outputs: vec![].into(),
            execution_domains: ExecutionDomains::from_parts(domains.into(), ancestors.into()),
        },
        WorkflowInputSchema::default(),
    )
}

#[test]
fn composes_external_secrets_and_repeats_only_the_selected_tool_in_isolated_scopes() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let results = seen.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let executed = calls.clone();
    let nodes = vec![
        node("config", vec![], vec![port("token")], |_, _| {
            Ok(values("token", "credential").into())
        }),
        node("parent", vec![], vec![], move |_, ctx| {
            assert_eq!(ctx.call_targets().len(), 2);
            for argument in ["first", "second"] {
                let response = ctx.invoke_call(
                    CallBindingId::new(0),
                    CallArguments {
                        outputs: values("query", argument),
                        ..Default::default()
                    },
                )?;
                let CallOutcome::Succeeded(outputs) = response.outcome else {
                    panic!("expected success")
                };
                results.lock().unwrap().push((response.id, outputs));
                assert_eq!(ctx.current_node(), Some("parent"));
                assert!(ctx.invocation_path().is_empty());
                assert!(ctx.output("tool.reply").is_err());
            }
            Ok(NodeResult::default())
        }),
        node(
            "tool",
            vec![port("text"), port("token")],
            vec![port("reply")],
            move |inputs, ctx| {
                executed.fetch_add(1, Ordering::SeqCst);
                assert!(inputs["token"].is_secret());
                assert_eq!(ctx.current_node(), Some("tool"));
                assert_eq!(ctx.invocation_path().len(), 1);
                assert!(ctx.call_targets().is_empty());
                Ok(Outputs::from([("reply".into(), inputs["text"].clone())]).into())
            },
        ),
        node("unused", vec![], vec![], |_, _| {
            panic!("unselected tool must not execute")
        }),
    ];
    let mut call = binding(1, 2, "search", "tool");
    call.binding.inputs = vec![
        ResolvedCallInput {
            port: port("text"),
            source: CallInputSource::Output(port("query")),
            secret: false,
        },
        ResolvedCallInput {
            port: port("token"),
            source: CallInputSource::External,
            secret: true,
        },
    ];
    call.binding.dependencies.push(FlowDependency {
        exclusive: false,
        input: Some("token".into()),
        source_node: "config".into(),
        source_output: "token".into(),
        source_key: "config.token".into(),
    });
    call.visibility = OutputVisibility::Borrowed(&["config.token"]);
    let mut unused = binding(1, 3, "unused", "unused");
    unused.binding.id = CallBindingId::new(1);
    let mut nodes = nodes;
    nodes[2].metadata.input_bindings.insert(
        "token".into(),
        InputBinding {
            secret: true,
            ..Default::default()
        },
    );
    let flow = flow(nodes, &[0, 1], true)
        .with_prepared_calls(vec![call, unused])
        .unwrap();
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let captured = snapshots.clone();
    let mut ctx = ExecutionContext::default();
    ctx.set_snapshot_recorder(SnapshotRecorder::new(move |event| {
        captured.lock().unwrap().push(event)
    }));
    flow.execute_in_context(&mut ctx).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let results = seen.lock().unwrap();
    assert_ne!(results[0].0, results[1].0);
    assert_eq!(results[0].1["reply"], ValueRef::from("first"));
    assert_eq!(results[1].1["reply"], ValueRef::from("second"));
    assert!(ctx.call_targets().is_empty());
    assert!(matches!(
        ctx.invoke_call(CallBindingId::new(0), CallArguments::default()),
        Err(CallDispatchError::Unavailable)
    ));
    let snapshots = snapshots.lock().unwrap();
    let scopes: BTreeSet<_> = snapshots
        .iter()
        .filter(|event| event.node.as_ref() == "tool")
        .map(|event| event.scope.clone())
        .collect();
    assert_eq!(scopes.len(), 2);
    assert!(scopes.iter().all(|scope| matches!(scope.as_slice(), [ScopePathEntry::Invocation { target_id, .. }] if target_id == "tool")));
}

#[test]
fn nested_calls_share_unique_identities_and_restore_the_parent_capability() {
    let ids = Arc::new(Mutex::new(Vec::new()));
    let parent_ids = ids.clone();
    let nested_ids = ids.clone();
    let nodes = vec![
        node("parent", vec![], vec![], move |_, ctx| {
            for _ in 0..2 {
                let response = ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
                assert!(matches!(response.outcome, CallOutcome::Succeeded(_)));
                parent_ids.lock().unwrap().push(response.id.invocation);
                assert_eq!(ctx.call_targets()[0].name(), "outer");
            }
            Ok(NodeResult::default())
        }),
        node("outer", vec![], vec![], move |_, ctx| {
            assert_eq!(ctx.call_targets()[0].name(), "inner");
            let response = ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
            nested_ids.lock().unwrap().push(response.id.invocation);
            assert_eq!(ctx.invocation_path().len(), 1);
            Ok(NodeResult::default())
        }),
        node("inner", vec![], vec![], |_, ctx| {
            assert_eq!(ctx.invocation_path().len(), 2);
            assert_eq!(ctx.current_node(), Some("inner"));
            Ok(NodeResult::default())
        }),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![
            binding(0, 1, "outer", "outer"),
            binding(1, 2, "inner", "inner"),
        ])
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(
        ids.lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
}

#[test]
fn skipped_external_inputs_skip_only_the_selected_invocation() {
    let nodes = vec![
        node(
            "config",
            vec![],
            vec![PortSpec::new("token", ValueType::String, false)],
            |_, _| {
                Ok(NodeResult {
                    skipped: BTreeSet::from(["token".into()]),
                    ..Default::default()
                })
            },
        ),
        node("parent", vec![], vec![], |_, ctx| {
            let response = ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
            assert!(
                matches!(response.outcome, CallOutcome::Skipped { causes } if causes.len() == 1)
            );
            Ok(NodeResult::default())
        }),
        node("tool", vec![port("token")], vec![], |_, _| {
            panic!("skipped invocation")
        }),
    ];
    let mut call = binding(1, 2, "tool", "tool");
    call.binding.inputs.push(ResolvedCallInput {
        port: port("token"),
        source: CallInputSource::External,
        secret: false,
    });
    call.binding.dependencies.push(FlowDependency {
        exclusive: false,
        input: Some("token".into()),
        source_node: "config".into(),
        source_output: "token".into(),
        source_key: "config.token".into(),
    });
    call.visibility = OutputVisibility::Borrowed(&["config.token"]);
    flow(nodes, &[0, 1], true)
        .with_prepared_calls(vec![call])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn serializes_shared_instances_and_nested_parallel_work_progresses_at_each_worker_limit() {
    for limit in [1, 2, 4] {
        let active = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let executed = calls.clone();
        let parent = |_, ctx: &mut ExecutionContext| {
            for _ in 0..3 {
                let response = ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
                assert!(matches!(response.outcome, CallOutcome::Succeeded(_)));
            }
            Ok(NodeResult::default())
        };
        let nodes = vec![
            node("left", vec![], vec![], parent),
            node("right", vec![], vec![], parent),
            node("tool", vec![], vec![], move |_, ctx| {
                assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                let thread = std::thread::current().id();
                assert_eq!(
                    ctx.run_parallel(2, |_| vec![|| std::thread::current().id(), || {
                        std::thread::current().id()
                    }])?,
                    [thread, thread]
                );
                std::thread::sleep(Duration::from_millis(2));
                active.fetch_sub(1, Ordering::SeqCst);
                executed.fetch_add(1, Ordering::SeqCst);
                Ok(NodeResult::default())
            }),
        ];
        flow(nodes, &[0, 1], false)
            .with_prepared_calls(vec![
                binding(0, 2, "tool", "tool"),
                binding(1, 2, "tool", "tool"),
            ])
            .unwrap()
            .execute_with_options(RuntimeOptions {
                max_parallel_domains: NonZeroUsize::new(limit).unwrap(),
            })
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 6);
    }
}

#[test]
fn discovery_overlays_restore_capabilities_and_reject_unknown_or_invalid_arguments() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            assert!(matches!(
                ctx.invoke_call(CallBindingId::new(99), CallArguments::default()),
                Err(CallDispatchError::UnknownBinding { .. })
            ));
            assert!(matches!(
                ctx.invoke_call(
                    CallBindingId::new(0),
                    CallArguments {
                        defaults: values("forged", "value"),
                        ..Default::default()
                    }
                ),
                Err(CallDispatchError::Arguments { .. })
            ));
            let interfaces =
                CallInterfaces::new([(DefinitionId::from("parent"), ctx.call_targets().to_vec())])
                    .unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _: Result<(), ()> = ctx.with_call_interfaces(&interfaces, |ctx| {
                    assert!(matches!(
                        ctx.invoke_call(CallBindingId::new(0), CallArguments::default()),
                        Err(CallDispatchError::Unavailable)
                    ));
                    panic!("restore capability");
                });
            }));
            assert!(result.is_err());
            assert!(matches!(
                ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?
                    .outcome,
                CallOutcome::Succeeded(_)
            ));
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], |_, _| Ok(NodeResult::default())),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn failure_outcomes_preserve_typed_sources_and_do_not_stop_the_parent() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            let response = ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
            let CallOutcome::Failed { source } = response.outcome else {
                panic!("expected failure")
            };
            let WorkflowRunError::NodeExecution {
                source: NodeExecutionError::PluginFailed { source },
                ..
            } = source
            else {
                panic!("expected source chain")
            };
            assert_eq!(
                source.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::NotFound
            );
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], |_, _| {
            Err(
                Box::<dyn std::error::Error + Send + Sync>::from(std::io::Error::from(
                    std::io::ErrorKind::NotFound,
                ))
                .into(),
            )
        }),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn deadlines_are_checked_before_and_after_execution_and_inherited_by_nested_calls() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            assert!(matches!(
                ctx.invoke_call_with_deadline(
                    CallBindingId::new(0),
                    CallArguments::default(),
                    Some(Instant::now())
                ),
                Err(CallDispatchError::DeadlineExceeded)
            ));
            let deadline = Instant::now() + Duration::from_millis(30);
            assert!(matches!(
                ctx.invoke_call_with_deadline(
                    CallBindingId::new(0),
                    CallArguments::default(),
                    Some(deadline)
                ),
                Err(CallDispatchError::DeadlineExceeded)
            ));
            assert_eq!(ctx.effective_deadline(None), None);
            Ok(NodeResult::default())
        }),
        node("outer", vec![], vec![], |_, ctx| {
            let deadline = ctx.effective_deadline(None).unwrap();
            assert_eq!(
                ctx.effective_deadline(Some(deadline + Duration::from_secs(10))),
                Some(deadline)
            );
            ctx.invoke_call_with_deadline(
                CallBindingId::new(0),
                CallArguments::default(),
                Some(deadline + Duration::from_secs(10)),
            )?;
            Ok(NodeResult::default())
        }),
        node("inner", vec![], vec![], |_, ctx| {
            let deadline = ctx.effective_deadline(None).unwrap();
            std::thread::sleep(
                deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(2),
            );
            Ok(NodeResult::default())
        }),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![
            binding(0, 1, "outer", "outer"),
            binding(1, 2, "inner", "inner"),
        ])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn cancellation_is_terminal_and_never_returns_successful_tool_outputs() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            assert!(matches!(
                ctx.invoke_call(CallBindingId::new(0), CallArguments::default()),
                Err(CallDispatchError::Cancelled { .. })
            ));
            assert!(matches!(
                ctx.invoke_call(CallBindingId::new(0), CallArguments::default()),
                Err(CallDispatchError::Cancelled { .. })
            ));
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![port("reply")], |_, ctx| {
            ctx.cancellation().cancel(StreamError::Execution {
                message: "stop".into(),
            });
            Ok(values("reply", "must not escape").into())
        }),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn poisoned_instances_cannot_be_reused_after_a_panic() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ctx.invoke_call(CallBindingId::new(0), CallArguments::default())
            }));
            assert!(panic.is_err());
            assert_eq!(ctx.current_node(), Some("parent"));
            assert!(matches!(
                ctx.invoke_call(CallBindingId::new(0), CallArguments::default()),
                Err(CallDispatchError::Poisoned)
            ));
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], |_, _| panic!("tool panic")),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn retaining_a_child_context_does_not_retain_its_executor_graph() {
    let lifetime = Arc::new(());
    let owned = lifetime.clone();
    let retained = Arc::new(Mutex::new(None));
    let slot = retained.clone();
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], move |_, ctx| {
            assert_eq!(Arc::strong_count(&owned), 2);
            *slot.lock().unwrap() = Some(ctx.fork_domain_with_observation(None));
            Ok(NodeResult::default())
        }),
    ];
    let flow = flow(nodes, &[0], true)
        .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
        .unwrap();
    flow.execute().unwrap();
    drop(flow);
    assert_eq!(Arc::strong_count(&lifetime), 1);
    assert!(matches!(
        retained
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .invoke_call(CallBindingId::new(0), CallArguments::default()),
        Err(CallDispatchError::Unavailable)
    ));
}

#[test]
fn waiting_for_a_busy_instance_obeys_the_invocation_deadline() {
    let entered = Arc::new(AtomicUsize::new(0));
    let waiting = entered.clone();
    let target = entered.clone();
    let nodes = vec![
        node("owner", vec![], vec![], |_, ctx| {
            assert!(matches!(
                ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?
                    .outcome,
                CallOutcome::Succeeded(_)
            ));
            Ok(NodeResult::default())
        }),
        node("waiter", vec![], vec![], move |_, ctx| {
            let timeout = Instant::now() + Duration::from_secs(2);
            while waiting.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < timeout);
                std::thread::yield_now();
            }
            assert!(matches!(
                ctx.invoke_call_with_deadline(
                    CallBindingId::new(0),
                    CallArguments::default(),
                    Some(Instant::now() + Duration::from_millis(5))
                ),
                Err(CallDispatchError::DeadlineExceeded)
            ));
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], move |_, _| {
            target.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            Ok(NodeResult::default())
        }),
    ];
    flow(nodes, &[0, 1], false)
        .with_prepared_calls(vec![
            binding(0, 2, "tool", "tool"),
            binding(1, 2, "tool", "tool"),
        ])
        .unwrap()
        .execute_with_options(RuntimeOptions {
            max_parallel_domains: NonZeroUsize::new(2).unwrap(),
        })
        .unwrap();
    assert_eq!(entered.load(Ordering::SeqCst), 1);
}

#[test]
fn prepared_tables_reject_ambiguous_routes_and_ordinary_target_activation() {
    let nodes = || {
        vec![
            node("parent", vec![], vec![], |_, _| Ok(NodeResult::default())),
            node("tool", vec![], vec![], |_, _| Ok(NodeResult::default())),
        ]
    };
    for (caller, target, identity, index, scheduled) in [
        (5, 1, "tool", 0, vec![0]),
        (0, 5, "tool", 0, vec![0]),
        (0, 1, "wrong", 0, vec![0]),
        (0, 1, "tool", 1, vec![0]),
        (0, 1, "tool", 0, vec![0, 1]),
    ] {
        let mut call = binding(caller, target, "tool", identity);
        call.binding.id = CallBindingId::new(index);
        assert!(matches!(
            flow(nodes(), &scheduled, true).with_prepared_calls(vec![call]),
            Err(CallDispatchError::InvalidPlan { .. })
        ));
    }
    let mut duplicate = binding(0, 1, "tool", "tool");
    duplicate.binding.id = CallBindingId::new(1);
    assert!(matches!(
        flow(nodes(), &[0], true)
            .with_prepared_calls(vec![binding(0, 1, "tool", "tool"), duplicate]),
        Err(CallDispatchError::Interfaces { .. })
    ));
    flow(nodes(), &[0, 1], true)
        .with_prepared_calls(vec![])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn success_prerequisites_and_missing_external_values_keep_their_existing_outcomes() {
    for missing in [false, true] {
        let nodes = vec![
            node("parent", vec![], vec![], move |_, ctx| {
                let response = ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
                match response.outcome {
                    CallOutcome::Skipped { causes } if !missing => assert_eq!(
                        causes,
                        [SkipCause::NodeSuccess {
                            node_success: "producer".into()
                        }]
                    ),
                    CallOutcome::Failed {
                        source: WorkflowRunError::Dependency { .. },
                    } if missing => {}
                    outcome => panic!("unexpected outcome: {outcome:?}"),
                }
                Ok(NodeResult::default())
            }),
            node("tool", vec![], vec![], |_, _| {
                panic!("prerequisites must prevent execution")
            }),
        ];
        let mut call = binding(0, 1, "tool", "tool");
        if missing {
            call.binding.dependencies.push(FlowDependency {
                exclusive: false,
                input: None,
                source_node: "producer".into(),
                source_output: "ready".into(),
                source_key: "producer.ready".into(),
            });
        } else {
            call.guard = Arc::new(|_| {
                Ok(BTreeSet::from([SkipCause::NodeSuccess {
                    node_success: "producer".into(),
                }]))
            });
        }
        flow(nodes, &[0], true)
            .with_prepared_calls(vec![call])
            .unwrap()
            .execute()
            .unwrap();
    }
}

#[test]
fn invalid_recursive_prepared_tables_fail_without_reentering_a_locked_executor() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            assert!(
                matches!(ctx.invoke_call(CallBindingId::new(0), CallArguments::default()), Err(CallDispatchError::Execution { source }) if source.is_terminal_control())
            );
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], |_, ctx| {
            ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
            Ok(NodeResult::default())
        }),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![
            binding(0, 1, "tool", "tool"),
            binding(1, 1, "recursive", "tool"),
        ])
        .unwrap()
        .execute()
        .unwrap();
}

#[cfg(feature = "async")]
#[test]
fn asynchronous_tool_operations_cannot_extend_the_inherited_deadline() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            assert!(matches!(
                ctx.invoke_call_with_deadline(
                    CallBindingId::new(0),
                    CallArguments::default(),
                    Some(Instant::now() + Duration::from_millis(20))
                ),
                Err(CallDispatchError::DeadlineExceeded)
            ));
            Ok(NodeResult::default())
        }),
        node("tool", vec![], vec![], |_, ctx| {
            ctx.check_future_limits(None)?;
            ctx.execute_future(
                std::future::pending::<()>(),
                Some(Instant::now() + Duration::from_secs(10)),
            )?;
            panic!("pending operation must expire")
        }),
    ];
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn default_arguments_distinguish_null_from_omission_without_retaining_previous_outputs() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, ctx| {
            for present in [true, false] {
                let response = ctx.invoke_call(
                    CallBindingId::new(0),
                    CallArguments {
                        defaults: if present {
                            Outputs::from([("value".into(), serde_json::Value::Null.into())])
                        } else {
                            Outputs::new()
                        },
                        ..Default::default()
                    },
                )?;
                let CallOutcome::Succeeded(outputs) = response.outcome else {
                    panic!("expected success")
                };
                assert_eq!(outputs.contains_key("value"), present);
            }
            Ok(NodeResult::default())
        }),
        node(
            "tool",
            vec![PortSpec::new(
                "value",
                ValueType::Nullable(Box::new(ValueType::String)),
                false,
            )],
            vec![PortSpec::new("value", ValueType::Any, false)],
            |inputs, _| Ok(inputs.into()),
        ),
    ];
    let mut call = binding(0, 1, "tool", "tool");
    call.binding.inputs.push(ResolvedCallInput {
        port: PortSpec::new(
            "value",
            ValueType::Nullable(Box::new(ValueType::String)),
            false,
        ),
        source: CallInputSource::Default,
        secret: false,
    });
    flow(nodes, &[0], true)
        .with_prepared_calls(vec![call])
        .unwrap()
        .execute()
        .unwrap();
}

#[test]
fn prepared_calls_reject_overlapping_caller_and_external_suppliers() {
    let nodes = vec![
        node("parent", vec![], vec![], |_, _| Ok(NodeResult::default())),
        node("tool", vec![port("value")], vec![], |_, _| {
            Ok(NodeResult::default())
        }),
    ];
    let mut call = binding(0, 1, "tool", "tool");
    call.binding.inputs.push(ResolvedCallInput {
        port: port("value"),
        source: CallInputSource::Default,
        secret: false,
    });
    call.binding.dependencies.push(FlowDependency {
        exclusive: false,
        input: Some("value".into()),
        source_node: "other".into(),
        source_output: "value".into(),
        source_key: "other.value".into(),
    });
    assert!(matches!(
        flow(nodes, &[0], true).with_prepared_calls(vec![call]),
        Err(CallDispatchError::InvalidPlan { .. })
    ));
}

#[test]
fn observations_record_each_invocation_without_root_target_completion() {
    use mf_telemetry::{description::WorkflowDescription, identity::RunId, observation::Observer};
    use opentelemetry_sdk::{
        logs::{InMemoryLogExporter, SdkLoggerProvider},
        trace::SdkTracerProvider,
    };
    use serde_json::json;
    for failed in [false, true] {
        let nodes = vec![
            node("parent", vec![], vec![], |_, ctx| {
                for _ in 0..2 {
                    ctx.invoke_call(CallBindingId::new(0), CallArguments::default())?;
                }
                Ok(NodeResult::default())
            }),
            node("tool", vec![], vec![], move |_, _| {
                if failed {
                    Err(NodeExecutionError::ExecutionFailed {
                        message: "tool failure".into(),
                    })
                } else {
                    Ok(NodeResult::default())
                }
            }),
        ];
        let graph_nodes =
            json!([{"id":"parent","kind":"test.parent"},{"id":"tool","kind":"test.tool"}]);
        let description: WorkflowDescription = serde_json::from_value(json!({
            "version":"2026-10-28", "workflow_id":format!("sha256:{}", "a".repeat(64)),
            "execution":{"mode":"single"}, "nodes":graph_nodes, "execution_order":["parent","tool"], "data_edges":[], "control_edges":[],
            "loop_bodies":[{"path":["parent"],"nodes":graph_nodes,"execution_order":["parent","tool"],"data_edges":[],"control_edges":[],
                "call_targets":[{"binding":0,"name":"tool","target":"tool"}]}]
        })).unwrap();
        let exporter = InMemoryLogExporter::default();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let traces = SdkTracerProvider::default();
        let run = Observer::new(&traces, &logs)
            .start_with_description(description, RunId::new())
            .unwrap();
        flow(nodes, &[0], true)
            .with_prepared_calls(vec![binding(0, 1, "tool", "tool")])
            .unwrap()
            .execute_with_observation(Some(run))
            .unwrap();
        let emitted = exporter.get_emitted_logs().unwrap();
        let names: Vec<_> = emitted
            .iter()
            .filter_map(|log| log.record.event_name())
            .collect();
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "mf.call.started")
                .count(),
            2
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "mf.call.finished")
                .count(),
            2
        );
        let target_events: Vec<_> = emitted
            .iter()
            .filter(|log| {
                log.record.attributes_iter().any(|(key, value)| {
                    key.as_str() == "mf.node.id"
                        && *value == opentelemetry::logs::AnyValue::String("tool".into())
                })
            })
            .collect();
        assert_eq!(target_events.len(), 4);
        for event in target_events {
            let opentelemetry::logs::AnyValue::Map(body) = event.record.body().unwrap() else {
                panic!("expected structured body")
            };
            assert!(body.iter().any(|(key, _)| key.as_str() == "loop_path"));
        }
    }
}
