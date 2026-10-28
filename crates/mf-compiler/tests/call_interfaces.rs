extern crate mfn_core as _;
use mf_compiler::{FlowBuilder, NodeRegistry};
use mf_runtime::{
    CallBindingId, CallInterfaces, ExecutionContext, Flow, FlowNode, GeneratedDomainExecutor,
    GeneratedNodeResult, Inputs, NodeExecution, NodeExecutionError, NodeId, NodeMetadata,
    NodePortContract, NodePorts, NodeResult, NodeTarget, Outputs, PortSpec, PreparedNode,
    ResolvedCallBinding, SnapshotRecorder, StreamExecution, StreamOptions, TaskNode, ValueType,
    WorkflowArguments,
};
use serde_json::{Value, json};
use snafu::ResultExt;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn catalog(entries: &[(&str, &str)]) -> CallInterfaces {
    CallInterfaces::new(entries.iter().map(|(caller, alias)| {
        let binding = ResolvedCallBinding {
            description: None,
            id: CallBindingId::new(0),
            name: (*alias).into(),
            caller: NodeId::new(0),
            target: NodeId::new(1),
            inputs: vec![],
            dependencies: vec![],
            success_dependencies: vec![],
            prerequisites: vec![],
        };
        ((*caller).into(), vec![binding.interface()])
    }))
    .unwrap()
}

fn visible(ctx: &ExecutionContext) -> Result<mf_runtime::ValueRef, NodeExecutionError> {
    Ok(serde_json::to_value(ctx.call_targets())
        .boxed()
        .context(mf_runtime::NodePluginFailedSnafu)?
        .into())
}

struct Inspect {
    child: Option<Flow>,
}
impl TaskNode for Inspect {
    fn execute(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        let before = visible(ctx)?;
        let child = if let Some(flow) = &self.child {
            // A different Flow must override even an explicitly inherited domain catalog.
            let mut child = ctx.fork_domain_with_observation(None);
            flow.execute_in_context(&mut child)?["tools"].clone()
        } else {
            json!([]).into()
        };
        assert_eq!(visible(ctx)?, before);
        Ok(Outputs::from([("tools".into(), before), ("child".into(), child)]).into())
    }
}

fn metadata(inputs: Vec<PortSpec>) -> NodeMetadata {
    NodeMetadata {
        ports: NodePorts {
            inputs,
            outputs: vec![
                PortSpec::new("tools", ValueType::Array, true),
                PortSpec::new("child", ValueType::Array, true),
            ],
        },
        call_outputs: vec![PortSpec::new("shared", ValueType::Any, false)],
        ..Default::default()
    }
}

fn inspector(id: &str, child: Option<Flow>, inputs: Vec<PortSpec>) -> FlowNode {
    FlowNode::new(
        id,
        PreparedNode::from_parts(
            NodeExecution::Task(Box::new(Inspect { child })),
            metadata(inputs),
        ),
    )
}

fn prepare(
    mut nodes: Vec<FlowNode>,
    edges: Vec<mf_runtime::EdgeDefinition>,
    mut order: Vec<mf_runtime::DefinitionId>,
    outputs: Vec<mf_runtime::WorkflowOutputDefinition>,
    interfaces: CallInterfaces,
) -> Result<FlowBuilder, mf_compiler::CallBindingError> {
    let mut calls = serde_json::Map::new();
    for caller in interfaces.callers() {
        let aliases = interfaces
            .for_caller(caller)
            .iter()
            .map(|interface| (interface.name().into(), json!({"target":"unused_tool"})))
            .collect::<serde_json::Map<_, _>>();
        calls.insert(caller.to_string(), aliases.into());
    }
    if !calls.is_empty() {
        nodes.push(FlowNode::new(
            "unused_tool",
            empty_target(Value::Null).unwrap(),
        ));
        order.push("unused_tool".into());
    }
    let definition = serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{},
        "nodes":nodes.iter().map(|node| json!({"id":node.definition_id,"kind":"test.interface_empty"})).collect::<Vec<_>>(),
        "calls":calls,
    })).unwrap();
    FlowBuilder::prepare(nodes, edges, order, outputs)?.with_declared_calls(&definition)
}

fn single(child: Option<Flow>, interfaces: CallInterfaces) -> Flow {
    prepare(
        vec![inspector("parent", child, vec![])],
        vec![],
        vec!["parent".into()],
        serde_json::from_value(json!([
            {"name":"tools","node":"parent","port":"tools"},
            {"name":"child","node":"parent","port":"child"}
        ]))
        .unwrap(),
        interfaces,
    )
    .unwrap()
    .into_tasks()
    .unwrap()
}

fn inspector_factory(_: Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(Inspect { child: None })),
        metadata(vec![]),
    ))
}

struct NeverRun;
impl TaskNode for NeverRun {
    fn execute(
        &self,
        _: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        panic!("interface preparation must not invoke a target")
    }
}
fn empty_target(_: Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(NeverRun)),
        NodeMetadata::default(),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind:"test.interface_empty", factory:mf_runtime::NodeFactory::Plain(empty_target) } }
fn target_factory(_: Value) -> Result<PreparedNode, mf_runtime::NodeBuildError> {
    Ok(PreparedNode::from_parts(
        NodeExecution::Task(Box::new(NeverRun)),
        NodeMetadata::new(NodePorts {
            inputs: vec![
                PortSpec::new("provided", ValueType::String, true),
                PortSpec::new("credential", ValueType::String, true),
            ],
            outputs: vec![],
        }),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind:"test.interface_inspector", factory:mf_runtime::NodeFactory::Plain(inspector_factory) } }
inventory::submit! { mf_runtime::NodeRegistration { kind:"test.interface_target", factory:mf_runtime::NodeFactory::Plain(target_factory) } }

#[test]
fn compiled_catalogs_prepare_all_callers_together_without_advertising_external_inputs() {
    let definition = serde_json::from_value(json!({
        "version":"2026-10-28", "dependencies":{}, "nodes":[
            {"id":"left","kind":"test.interface_inspector"},
            {"id":"right","kind":"test.interface_inspector"},
            {"id":"config","kind":"builtin.constant","config":{"value":"never publish this value"}},
            {"id":"left_tool","kind":"test.interface_target","inputs":{"credential":{"from":{"node":"config","port":"value"}}}},
            {"id":"right_tool","kind":"test.interface_target","inputs":{"credential":{"from":{"node":"config","port":"value"}}}}
        ], "calls": {
            "left":{"left_tool":{"target":"left_tool","ports":[{"output":"shared","input":"provided"}]}},
            "right":{"right_tool":{"target":"right_tool","ports":[{"output":"shared","input":"provided"}]}}
        }, "outputs":[
            {"name":"left","node":"left","port":"tools"}, {"name":"right","node":"right","port":"tools"}
        ]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    for name in ["left", "right"] {
        let view = &flow.call_interfaces().for_caller(&name.into())[0];
        assert_eq!(view.outputs()[0].name(), "shared");
        assert!(view.outputs()[0].required());
        let json = serde_json::to_string(view).unwrap();
        for private in ["credential", "config", "provided", "never publish"] {
            assert!(!json.contains(private));
        }
    }
    for limit in [1, 4] {
        let outputs = flow
            .execute_with_options(mf_runtime::RuntimeOptions {
                max_parallel_domains: std::num::NonZeroUsize::new(limit).unwrap(),
            })
            .unwrap();
        assert_eq!(outputs["left"][0]["name"], json!("left_tool"));
        assert_eq!(outputs["right"][0]["name"], json!("right_tool"));
    }
}

#[test]
fn generated_domains_and_snapshot_fallback_observe_the_same_interfaces() {
    let generated = Arc::new(AtomicUsize::new(0));
    let observed = generated.clone();
    let executor: Arc<GeneratedDomainExecutor> = Arc::new(move |flow, ctx| {
        assert!(ctx.call_targets().is_empty());
        observed.fetch_add(1, Ordering::SeqCst);
        flow.execute_generated_position(0, &[], ctx, |inputs, ctx| {
            Inspect { child: None }
                .execute(inputs, ctx)
                .map(GeneratedNodeResult::Dynamic)
        })
    });
    let flow =
        single(None, catalog(&[("parent", "search")])).with_generated_domains([(0, executor)]);
    let ordinary = flow.execute().unwrap();
    assert_eq!(ordinary["tools"][0]["name"], json!("search"));
    assert_eq!(generated.load(Ordering::SeqCst), 1);
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let captured = snapshots.clone();
    let mut ctx = ExecutionContext::default();
    ctx.set_snapshot_recorder(SnapshotRecorder::new(move |event| {
        captured.lock().unwrap().push(event)
    }));
    assert_eq!(flow.execute_in_context(&mut ctx).unwrap(), ordinary);
    assert_eq!(generated.load(Ordering::SeqCst), 1);
    assert!(!snapshots.lock().unwrap().is_empty());
    assert!(ctx.call_targets().is_empty());
    // Public direct dispatch also installs the owning Flow's scope.
    flow.execute_position(0, &mut ctx).unwrap();
    assert!(ctx.call_targets().is_empty());
}

#[test]
fn nested_flows_override_inherited_catalogs_and_restore_the_host_scope() {
    for child_has_catalog in [false, true] {
        let child = single(
            None,
            if child_has_catalog {
                catalog(&[("parent", "inner")])
            } else {
                CallInterfaces::default()
            },
        );
        let outer = single(Some(child), catalog(&[("parent", "outer")]));
        let mut ctx = ExecutionContext::default();
        ctx.with_call_interfaces(&catalog(&[("parent", "host")]), |ctx| {
            ctx.with_node("parent".into(), |ctx| {
                let result = outer.execute_in_context(ctx)?;
                assert_eq!(result["tools"][0]["name"], json!("outer"));
                if child_has_catalog {
                    assert_eq!(result["child"][0]["name"], json!("inner"));
                } else {
                    assert_eq!(result["child"], json!([]));
                }
                assert_eq!(ctx.call_targets()[0].name(), "host");
                assert_eq!(ctx.current_node(), Some("parent"));
                Ok::<_, mf_runtime::WorkflowRunError>(())
            })
        })
        .unwrap();
    }
    assert!(
        prepare(
            vec![inspector("parent", None, vec![])],
            vec![],
            vec!["parent".into()],
            vec![],
            catalog(&[("unknown", "tool")]),
        )
        .is_err()
    );
}

type Seen = Arc<Mutex<Vec<(String, Value)>>>;
struct Combined {
    root: bool,
    seen: Seen,
}
impl NodePortContract for Combined {
    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: if self.root {
                vec![]
            } else {
                vec![PortSpec::new("trigger", ValueType::String, true)]
            },
            outputs: vec![PortSpec::new("tools", ValueType::Array, true)],
        }
    }
    fn stream_outputs(&self) -> Vec<PortSpec> {
        vec![PortSpec::new("item", ValueType::String, true)]
    }
}
impl mf_runtime::TaskStreamNode for Combined {
    fn execute(
        &self,
        _: Inputs,
        ctx: &mut ExecutionContext,
        emitter: &mut mf_runtime::Emitter<'_>,
    ) -> Result<NodeResult, NodeExecutionError> {
        let tools = visible(ctx)?;
        self.seen.lock().unwrap().push((
            ctx.current_node().unwrap().into(),
            serde_json::to_value(&tools).unwrap(),
        ));
        emitter.send(Outputs::from([("item".into(), "tick".into())]).into())?;
        Ok(Outputs::from([("tools".into(), tools)]).into())
    }
}

#[test]
fn stream_messages_and_combined_producers_receive_the_owning_scope() {
    let seen: Seen = Arc::default();
    let nodes = vec![
        FlowNode::new(
            "source",
            PreparedNode::task_stream(
                Combined {
                    root: true,
                    seen: seen.clone(),
                },
                NodeMetadata::default(),
            )
            .unwrap(),
        ),
        FlowNode::new(
            "middle",
            PreparedNode::task_stream(
                Combined {
                    root: false,
                    seen: seen.clone(),
                },
                NodeMetadata::default(),
            )
            .unwrap(),
        ),
        inspector(
            "leaf",
            None,
            vec![PortSpec::new("trigger", ValueType::String, true)],
        ),
    ];
    let prepared = prepare(
        nodes,
        serde_json::from_value(json!([
            {"from_node":"source","from_output":"item","to_node":"middle","to_input":"trigger"},
            {"from_node":"middle","from_output":"item","to_node":"leaf","to_input":"trigger"}
        ]))
        .unwrap(),
        vec!["source".into(), "middle".into(), "leaf".into()],
        serde_json::from_value(json!([
            {"name":"tools","node":"leaf","port":"tools"}
        ]))
        .unwrap(),
        catalog(&[
            ("source", "source_tool"),
            ("middle", "middle_tool"),
            ("leaf", "leaf_tool"),
        ]),
    )
    .unwrap()
    .into_stream(StreamExecution::default())
    .unwrap();
    let leaf = prepared
        .plan()
        .execution_domains()
        .domains()
        .iter()
        .find(|domain| domain.nodes.contains(&NodeId::new(2)))
        .unwrap();
    let domain_id = leaf.id;
    let positions = leaf.positions.clone();
    let generated = Arc::new(AtomicUsize::new(0));
    let recorded = generated.clone();
    let executor: Arc<mf_runtime::GeneratedStreamDomainExecutor> = Arc::new(move |plan, ctx| {
        assert!(ctx.call_targets().is_empty());
        recorded.fetch_add(1, Ordering::SeqCst);
        for &position in positions.iter() {
            plan.execute_resolved_position(position, ctx, |_, ctx, inputs| {
                plan.resolve_inputs(position, ctx, inputs, Default::default())
            })?;
        }
        Ok(())
    });
    let prepared = prepared.with_prepared_domains([(domain_id, executor)]);
    let instance = prepared
        .start_with_options(StreamOptions::default())
        .unwrap();
    let result = instance.recv().unwrap().unwrap();
    assert_eq!(result.outputs["tools"][0]["name"], json!("leaf_tool"));
    assert!(instance.recv().unwrap().is_none());
    instance.join().unwrap();
    assert_eq!(generated.load(Ordering::SeqCst), 1);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for (name, tools) in seen.iter() {
        assert_eq!(tools[0]["name"], json!(format!("{name}_tool")));
    }
}

struct NativeSource;
impl mf_runtime::StreamNode for NativeSource {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        panic!("catalog attachment must not start a source")
    }
}

#[test]
fn stream_catalogs_reject_non_task_callers_before_starting_sources() {
    let source = FlowNode::new(
        "source",
        PreparedNode::from_parts(
            NodeExecution::Stream(Box::new(NativeSource)),
            NodeMetadata::new(NodePorts {
                inputs: vec![],
                outputs: vec![PortSpec::new("item", ValueType::String, true)],
            }),
        ),
    );
    assert!(
        prepare(
            vec![source],
            vec![],
            vec!["source".into()],
            vec![],
            catalog(&[("source", "tool")]),
        )
        .is_err()
    );
}

#[test]
fn isolated_targets_use_their_prepared_scope_without_retaining_it_in_the_host() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition =
        serde_json::from_value(json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"parent","kind":"test.interface_inspector"},
        {"id":"unused_tool","kind":"test.interface_empty"}
    ],"calls":{"parent":{"isolated":{"target":"unused_tool"}}}}))
        .unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    let nodes = mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap();
    let targets = plan.node_targets(&nodes).unwrap();
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    let selected = NodeTarget::find(&targets, "parent")
        .unwrap()
        .bind_flow(&flow)
        .unwrap();
    let mut ctx = ExecutionContext::default();
    let output = mf_runtime::FlowRuntime::default()
        .execute_target_in_context(
            &selected,
            &mut ctx,
            WorkflowArguments::default(),
            WorkflowArguments::default(),
        )
        .unwrap();
    assert_eq!(output["tools"][0]["name"], json!("isolated"));
    assert!(ctx.call_targets().is_empty());
    let stream = prepare(
        vec![inspector("parent", None, vec![])],
        vec![],
        vec!["parent".into()],
        vec![],
        catalog(&[("parent", "stream_isolated")]),
    )
    .unwrap()
    .into_stream(StreamExecution::default())
    .unwrap();
    let mut record = targets[0].clone();
    record.worker_limit = std::num::NonZeroUsize::new(stream.plan().execution().limits.workers);
    let selected = record.bind_stream(&stream).unwrap();
    let output = mf_runtime::FlowRuntime::default()
        .execute_target(
            &selected,
            WorkflowArguments::default(),
            WorkflowArguments::default(),
        )
        .unwrap();
    assert_eq!(output["tools"][0]["name"], json!("stream_isolated"));
}
