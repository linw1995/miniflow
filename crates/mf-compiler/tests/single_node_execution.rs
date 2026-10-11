#![cfg(feature = "codegen")]
extern crate mfn_code as _;
extern crate mfn_core as _;

use mf_compiler::{
    CompiledWorkflow, NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled,
};
use mf_runtime::{FlowRuntime, Inputs, NodeMetadata, NodeTarget, Outputs, WorkflowArguments};
use serde_json::{Value, json};
use snafu::ResultExt;

struct Tripwire;
impl mf_runtime::TaskNode for Tripwire {
    fn execute(
        &self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        panic!("isolated invocation executed an unrelated task")
    }
}
fn tripwire(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Tripwire)),
        NodeMetadata::new(mf_runtime::NodePorts {
            inputs: vec![mf_runtime::PortSpec::new(
                "input",
                mf_runtime::ValueType::Any,
                false,
            )],
            outputs: vec![mf_runtime::PortSpec::new(
                "value",
                mf_runtime::ValueType::Int64,
                true,
            )],
        }),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind:"test.isolation_tripwire", factory:mf_runtime::NodeFactory::Plain(tripwire) } }
struct Producer;
impl mf_runtime::StreamNode for Producer {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), mf_runtime::NodeExecutionError> {
        panic!("isolated task started its producer")
    }
}
fn producer(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let mut node = tripwire(Value::Null)?;
    node.execution = mf_runtime::NodeExecution::Stream(Box::new(Producer));
    Ok(node)
}
inventory::submit! { mf_runtime::NodeRegistration { kind:"test.isolation_producer", factory:mf_runtime::NodeFactory::Plain(producer) } }
fn plan(value: Value, registry: &NodeRegistry) -> CompiledWorkflow {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    compile_definition(&definition, registry).unwrap()
}
fn targets(plan: &CompiledWorkflow, registry: &NodeRegistry) -> Vec<NodeTarget> {
    serde_json::from_str(
        &plan
            .generate_execution_plans(registry)
            .unwrap()
            .node_targets_json,
    )
    .unwrap()
}
fn args(value: Value) -> WorkflowArguments {
    WorkflowArguments::try_from(value).unwrap()
}
fn chain(stream: bool) -> Value {
    let mut value = json!({"version":"2026-10-03", "dependencies":{},
        "nodes":[{"id":"before", "kind":if stream {"test.isolation_producer"} else {"test.isolation_tripwire"}}, {"id":"target", "kind":"builtin.identity"}, {"id":"after", "kind":"test.isolation_tripwire"}],
        "edges":[{"from_node":"before", "from_output":"value", "to_node":"target", "to_input":"input"}, {"from_node":"target", "from_output":"value", "to_node":"after", "to_input":"input"}],
        "control_edges":[{"from_node":"before", "from_output":"value", "to_node":"target"}],
        "outputs":[{"name":"alias", "node":"after", "port":"value"}]
    });
    if stream {
        value["execution"] = json!({"mode":"stream"});
    }
    value
}

#[test]
fn finite_selection_bypasses_edges_domain_executors_and_aliases() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = plan(chain(false), &registry);
    let records = targets(&plan, &registry);
    let flow = instantiate_compiled(&plan, &registry)
        .unwrap()
        .with_generated_domains([(
            0,
            std::sync::Arc::new(
                |_: &mf_runtime::Flow,
                 _: &mut mf_runtime::ExecutionContext|
                 -> Result<(), mf_runtime::WorkflowRunError> {
                    panic!("isolated target entered its generated domain")
                },
            ) as std::sync::Arc<mf_runtime::GeneratedDomainExecutor>,
        )]);
    let target = NodeTarget::find(&records, "target")
        .unwrap()
        .bind_flow(&flow)
        .unwrap();
    let runtime = FlowRuntime::default();
    for value in [21, 43] {
        let outputs = runtime
            .execute_target(
                &target,
                args(json!({"target.input":value})),
                WorkflowArguments::default(),
            )
            .unwrap();
        assert_eq!(json!(outputs), json!({"value":value}));
    }
    let error = runtime
        .execute_target(
            &target,
            args(json!({"target.input":"wrong"})),
            WorkflowArguments::default(),
        )
        .unwrap_err();
    assert!(
        matches!(error, mf_runtime::WorkflowRunError::OutputType { source, .. } if source.expected == mf_runtime::ValueType::Int64)
    );
    assert!(
        runtime
            .execute_target(
                &target,
                args(json!({"before.input":1})),
                WorkflowArguments::default()
            )
            .is_err()
    );
}

#[test]
fn streaming_selection_invokes_no_source_or_neighbor() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = plan(chain(true), &registry);
    let records = targets(&plan, &registry);
    let stream = mf_compiler::instantiate_stream(&plan, &registry).unwrap();
    let target = NodeTarget::find(&records, "target")
        .unwrap()
        .bind_stream(&stream)
        .unwrap();
    assert_eq!(
        json!(
            FlowRuntime::default()
                .execute_target(
                    &target,
                    args(json!({"target.input":42})),
                    WorkflowArguments::default()
                )
                .unwrap()
        ),
        json!({"value":42})
    );
}

#[test]
fn routers_read_only_explicit_context_and_omit_skipped_outputs() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = plan(
        json!({"version":"2026-10-03", "dependencies":{},
            "nodes":[{"id":"source", "kind":"test.isolation_tripwire"}, {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"yes", "condition":{"source":{"output":"source.value", "path":""}, "operator":"gt", "value":0}}]}}],
            "control_edges":[{"from_node":"source", "from_output":"value", "to_node":"route"}]
        }),
        &registry,
    );
    let records = targets(&plan, &registry);
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let target = NodeTarget::find(&records, "route")
        .unwrap()
        .bind_flow(&flow)
        .unwrap();
    for (value, expected) in [(1, json!({"yes":true})), (-1, json!({"else":true}))] {
        assert_eq!(
            json!(
                FlowRuntime::default()
                    .execute_target(
                        &target,
                        WorkflowArguments::default(),
                        args(json!({"source.value":value}))
                    )
                    .unwrap()
            ),
            expected
        );
    }
    assert!(
        FlowRuntime::default()
            .execute_target(
                &target,
                WorkflowArguments::default(),
                WorkflowArguments::default()
            )
            .is_err()
    );
}

#[test]
fn selected_loops_keep_pass_and_stop_rules() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let value: Value = serde_json::from_str(include_str!("../../../examples/loop.json")).unwrap();
    for (maximum, exit, expected) in [(5, false, 3), (1, false, 1), (5, true, 1)] {
        let mut value = value.clone();
        value["nodes"][1]["loop"]["max_iterations"] = json!(maximum);
        if exit {
            value["nodes"][1]["loop"]["body"]["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"aa_exit", "kind":"workflow.exit_loop"}));
        }
        let plan = plan(value, &registry);
        let records = targets(&plan, &registry);
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        assert_eq!(json!(flow.execute().unwrap()), json!({"count":expected}));
        let target = NodeTarget::find(&records, "repeat")
            .unwrap()
            .bind_flow(&flow)
            .unwrap();
        let outputs = FlowRuntime::default()
            .execute_target(
                &target,
                args(json!({"repeat.count":0})),
                WorkflowArguments::default(),
            )
            .unwrap();
        assert_eq!(json!(outputs), json!({"count":expected}));
    }
}

struct Item;
impl mf_runtime::TaskNode for Item {
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        assert_eq!(ctx.worker_limit().get(), 2);
        if inputs["item"].as_i64() == Some(0) {
            return mf_runtime::NodeExecutionFailedSnafu {
                message: "zero is rejected",
            }
            .fail();
        }
        Ok(Outputs::from([(
            "value".into(),
            (inputs["item"].as_i64().unwrap() * 2 + inputs["index"].as_i64().unwrap()).into(),
        )])
        .into())
    }
}
fn item(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Item)),
        NodeMetadata::new(mf_runtime::NodePorts {
            inputs: vec![
                mf_runtime::PortSpec::new("item", mf_runtime::ValueType::Int64, true),
                mf_runtime::PortSpec::new("index", mf_runtime::ValueType::Int64, true),
            ],
            outputs: vec![mf_runtime::PortSpec::new(
                "value",
                mf_runtime::ValueType::Int64,
                true,
            )],
        }),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration {kind:"test.isolation_item", factory:mf_runtime::NodeFactory::Plain(item)} }
#[test]
fn selected_parallel_iteration_keeps_order_worker_bounds_and_error_policy() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for policy in ["terminate", "continue_on_error", "remove_failed"] {
        let plan = plan(
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[
            {"id":"source", "kind":"builtin.constant", "config":{"value":[1,2,3]}},
            {"id":"iteration", "kind":"builtin.iteration", "config":{"mode":"parallel", "on_error":policy, "body":{
                "nodes":[{"id":"map", "kind":"test.isolation_item"}], "edges":[
                    {"from_node":"%iteration", "from_output":"item", "to_node":"map", "to_input":"item"},
                    {"from_node":"%iteration", "from_output":"index", "to_node":"map", "to_input":"index"}],
                "result":{"node":"map", "port":"value"}
            }}}
        ], "edges":[{"from_node":"source", "from_output":"value", "to_node":"iteration", "to_input":"items"}]}),
            &registry,
        );
        let records = targets(&plan, &registry);
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        let target = NodeTarget::find(&records, "iteration")
            .unwrap()
            .bind_flow(&flow)
            .unwrap();
        let runtime = FlowRuntime::new(mf_runtime::RuntimeOptions {
            max_parallel_domains: std::num::NonZeroUsize::new(2).unwrap(),
        });
        assert_eq!(
            json!(
                runtime
                    .execute_target(
                        &target,
                        args(json!({"iteration.items":[3,2,1]})),
                        WorkflowArguments::default()
                    )
                    .unwrap()
            ),
            json!({"results":[6,5,4]})
        );
        let result = runtime.execute_target(
            &target,
            args(json!({"iteration.items":[1,0,3]})),
            WorkflowArguments::default(),
        );
        if policy == "terminate" {
            assert!(result.is_err());
        } else if policy == "continue_on_error" {
            assert_eq!(json!(result.unwrap()), json!({"results":[2,null,8]}));
        } else {
            assert_eq!(json!(result.unwrap()), json!({"results":[2,8]}));
        }
    }
}

struct Read;
impl mf_runtime::TaskNode for Read {
    fn execute(
        &self,
        inputs: Inputs,
        ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        let value = if let Some(value) = inputs.get("text") {
            assert!(ctx.stdin_line().is_err());
            value.clone()
        } else {
            ctx.stdin_line()
                .boxed()
                .context(mf_runtime::NodePluginFailedSnafu)?
                .unwrap_or_default()
                .into()
        };
        Ok(Outputs::from([("value".into(), value)]).into())
    }
}
fn read(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let mut metadata = NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![mf_runtime::PortSpec::new(
            "text",
            mf_runtime::ValueType::String,
            false,
        )],
        outputs: vec![mf_runtime::PortSpec::new(
            "value",
            mf_runtime::ValueType::String,
            true,
        )],
    });
    metadata.stdin = Some(mf_runtime::StdinRequirement::UnlessInput("text".into()));
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Read)),
        metadata,
    ))
}
inventory::submit! { mf_runtime::NodeRegistration {kind:"test.isolation_read", factory:mf_runtime::NodeFactory::Plain(read)} }
#[test]
fn selected_resource_validation_uses_only_target_requirements() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = plan(
        json!({"version":"2026-10-03", "dependencies":{}, "nodes":[{"id":"read", "kind":"test.isolation_read"}]}),
        &registry,
    );
    let records = targets(&plan, &registry);
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    let target = NodeTarget::find(&records, "read")
        .unwrap()
        .bind_flow(&flow)
        .unwrap();
    let runtime = FlowRuntime::default();
    assert!(
        runtime
            .execute_target(
                &target,
                WorkflowArguments::default(),
                WorkflowArguments::default()
            )
            .is_err()
    );
    assert_eq!(
        json!(
            runtime
                .execute_target(
                    &target,
                    args(json!({"read.text":"provided"})),
                    WorkflowArguments::default()
                )
                .unwrap()
        ),
        json!({"value":"provided"})
    );
    let mut file = tempfile::tempfile().unwrap();
    use std::io::{Seek, Write};
    writeln!(file, "stdin value").unwrap();
    file.rewind().unwrap();
    let mut state = mf_runtime::ExecutionContext::default();
    state.set_stdin(mf_runtime::TextInput::new(file));
    assert_eq!(
        json!(
            runtime
                .execute_target_in_context(
                    &target,
                    &mut state,
                    WorkflowArguments::default(),
                    WorkflowArguments::default()
                )
                .unwrap()
        ),
        json!({"value":"stdin value"})
    );
}

struct ResultNode {
    empty: bool,
}
impl mf_runtime::TaskNode for ResultNode {
    fn execute(
        &self,
        inputs: Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        if self.empty {
            return Ok(Outputs::new().into());
        }
        Ok(mf_runtime::NodeResult {
            outputs: Outputs::from([(
                "value".into(),
                if inputs.contains_key("bad") {
                    "wrong".into()
                } else {
                    mf_runtime::ValueRef::null()
                },
            )]),
            skipped: std::collections::BTreeSet::from(["skipped".into()]),
            ..Default::default()
        })
    }
}
fn result_node(config: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    let empty = config["empty"] == true;
    let ports = if empty {
        Default::default()
    } else {
        mf_runtime::NodePorts {
            inputs: vec![mf_runtime::PortSpec::new(
                "bad",
                mf_runtime::ValueType::Boolean,
                false,
            )],
            outputs: vec![
                mf_runtime::PortSpec::new("value", mf_runtime::ValueType::Null, true),
                mf_runtime::PortSpec::new("skipped", mf_runtime::ValueType::Int64, false),
                mf_runtime::PortSpec::new("omitted", mf_runtime::ValueType::Int64, false),
            ],
        }
    };
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(ResultNode { empty })),
        NodeMetadata::new(ports),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration {kind:"test.isolation_results", factory:mf_runtime::NodeFactory::Plain(result_node)} }
#[test]
fn isolated_results_preserve_null_skips_empty_maps_and_transactional_publication() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for empty in [false, true] {
        let plan = plan(
            json!({"version":"2026-10-03", "dependencies":{}, "nodes":[{"id":"result", "kind":"test.isolation_results", "config":{"empty":empty}}]}),
            &registry,
        );
        let records = targets(&plan, &registry);
        let flow = instantiate_compiled(&plan, &registry).unwrap();
        let target = NodeTarget::find(&records, "result")
            .unwrap()
            .bind_flow(&flow)
            .unwrap();
        assert_eq!(
            json!(
                FlowRuntime::default()
                    .execute_target(
                        &target,
                        WorkflowArguments::default(),
                        WorkflowArguments::default()
                    )
                    .unwrap()
            ),
            if empty {
                json!({})
            } else {
                json!({"value":null})
            }
        );
        if !empty {
            let mut state = mf_runtime::ExecutionContext::default();
            let error = FlowRuntime::default()
                .execute_target_in_context(
                    &target,
                    &mut state,
                    args(json!({"result.bad":true})),
                    WorkflowArguments::default(),
                )
                .unwrap_err();
            assert!(matches!(
                error,
                mf_runtime::WorkflowRunError::OutputType { .. }
            ));
            assert!(state.output("result.value").is_err());
        }
    }
}
