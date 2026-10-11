#![cfg(feature = "codegen")]
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{FlowNode, Inputs, NodeMetadata, NodeTarget, NodeTargetKind, Outputs};
use serde_json::{Value, json};
use snafu::ResultExt;
use std::sync::atomic::{AtomicUsize, Ordering};

static CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);
static EXECUTIONS: AtomicUsize = AtomicUsize::new(0);
struct Probe;
impl mf_runtime::TaskNode for Probe {
    fn execute(
        &self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
        EXECUTIONS.fetch_add(1, Ordering::SeqCst);
        Ok(Outputs::new().into())
    }
}
fn factory(config: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    CONSTRUCTIONS.fetch_add(1, Ordering::SeqCst);
    if config["fail"] == true {
        return Err(std::io::Error::other("unavailable unrelated resource"))
            .boxed()
            .context(mf_runtime::NodeFactoryFailedSnafu);
    }
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(Probe)),
        NodeMetadata::default(),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.target_probe", factory: mf_runtime::NodeFactory::Plain(factory) } }

fn definition() -> WorkflowDefinition {
    serde_json::from_value(json!({"version":"2026-10-03", "dependencies":{},
        "nodes":[
            {"id":"source", "kind":"builtin.constant", "config":{"value":42}},
            {"id":"copy", "kind":"builtin.identity"},
            {"id":"probe", "kind":"test.target_probe"},
            {"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"yes", "condition":{"source":{"output":"source.value", "path":""}, "operator":"gt", "value":0}}]}}
        ],
        "edges":[{"from_node":"source", "from_output":"value", "to_node":"copy", "to_input":"input"}],
        "control_edges":[{"from_node":"source", "from_output":"value", "to_node":"route"}]
    })).unwrap()
}

#[test]
fn target_records_reuse_preparation_and_bind_original_contracts() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    CONSTRUCTIONS.store(0, Ordering::SeqCst);
    EXECUTIONS.store(0, Ordering::SeqCst);
    let artifacts = plan.generate_execution_plans(&registry).unwrap();
    assert_eq!(CONSTRUCTIONS.load(Ordering::SeqCst), 1);
    assert_eq!(EXECUTIONS.load(Ordering::SeqCst), 0);
    let targets: Vec<NodeTarget> = serde_json::from_str(&artifacts.node_targets_json).unwrap();
    assert_eq!(
        targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        plan.execution_order
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
    );
    let copy = NodeTarget::find(&targets, "copy").unwrap();
    assert_eq!(
        copy.outputs["value"].value_type,
        mf_runtime::ValueType::Int64
    );
    let flow = instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(copy.bind_flow(&flow).unwrap().contract(), copy);
    assert_eq!(
        NodeTarget::find(&targets, "route").unwrap().context["source.value"].value_type,
        mf_runtime::ValueType::Int64
    );

    for change in ["inputs", "outputs", "references", "stdin"] {
        let nodes = flow
            .nodes()
            .iter()
            .map(|node| {
                let mut metadata = node.metadata.clone();
                if node.definition_id.as_str() == "copy" {
                    match change {
                        "inputs" => metadata.ports.inputs[0].required = false,
                        "outputs" => {
                            metadata.ports.outputs[0].value_type = mf_runtime::ValueType::String
                        }
                        "references" => metadata
                            .context_references
                            .push(mf_runtime::ContextReference::new("source.value", "changed")),
                        "stdin" => metadata.stdin = Some(mf_runtime::StdinRequirement::Always),
                        _ => unreachable!(),
                    }
                }
                mf_runtime::TaskFlowNode {
                    definition_id: node.definition_id.clone(),
                    metadata,
                    node: Box::new(Probe),
                }
            })
            .collect();
        let changed =
            mf_runtime::Flow::from_plan(nodes, flow.plan().clone(), flow.input_schema().clone());
        let error = copy.bind_flow(&changed).err().unwrap();
        assert!(error.to_string().contains("copy") && error.to_string().contains("drift"));
    }

    let mut unavailable = plan.clone();
    unavailable
        .definition
        .nodes
        .iter_mut()
        .find(|node| node.id.as_str() == "probe")
        .unwrap()
        .config = json!({"fail":true});
    assert!(
        instantiate_compiled(&unavailable, &registry)
            .err()
            .unwrap()
            .to_string()
            .contains("unavailable unrelated resource")
    );
    assert_eq!(EXECUTIONS.load(Ordering::SeqCst), 0);
}

struct Producer;
impl mf_runtime::StreamNode for Producer {
    fn execute(
        &mut self,
        _: Inputs,
        _: &mut mf_runtime::ExecutionContext,
        _: &mut mf_runtime::Emitter<'_>,
    ) -> Result<(), mf_runtime::NodeExecutionError> {
        panic!("target preparation must not start a producer")
    }
}
fn producer(_: Value) -> Result<mf_runtime::PreparedNode, mf_runtime::NodeBuildError> {
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Stream(Box::new(Producer)),
        NodeMetadata::new(mf_runtime::NodePorts {
            inputs: vec![],
            outputs: vec![mf_runtime::PortSpec::new(
                "value",
                mf_runtime::ValueType::Int64,
                true,
            )],
        }),
    ))
}
inventory::submit! { mf_runtime::NodeRegistration { kind: "test.target_producer", factory: mf_runtime::NodeFactory::Plain(producer) } }

#[test]
fn stream_records_bind_tasks_and_reject_producers_without_starting_them() {
    let definition = serde_json::from_value(json!({"version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[{"id":"source", "kind":"test.target_producer"}, {"id":"copy", "kind":"builtin.identity"}],
        "edges":[{"from_node":"source", "from_output":"value", "to_node":"copy", "to_input":"input"}]
    })).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let nodes = mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap();
    let mut nodes: Vec<FlowNode> = nodes;
    // Resolved output metadata is retained by the same construction used during compilation.
    let mut inference = mf_compiler::TypeInferenceState::default();
    inference.resolve_node(&mut nodes[0], &[]).unwrap();
    inference
        .resolve_node(
            &mut nodes[1],
            &[mf_runtime::ExecutionDependency {
                input: Some("input"),
                source_node: "source",
                source_output: "value",
            }],
        )
        .unwrap();
    let targets = plan.node_targets(&nodes).unwrap();
    let stream = mf_compiler::instantiate_stream(&plan, &registry).unwrap();
    let target = NodeTarget::find(&targets, "copy").unwrap();
    assert_eq!(
        target.bind_stream(&stream).unwrap().contract().outputs["value"].value_type,
        mf_runtime::ValueType::Int64
    );
    assert_eq!(targets[0].kind, NodeTargetKind::Stream);
    let mut changed = targets[0].clone();
    changed.kind = NodeTargetKind::Task;
    assert!(
        changed
            .bind_stream(&stream)
            .err()
            .unwrap()
            .to_string()
            .contains("executor category")
    );
    assert!(
        NodeTarget::find(&targets, "source")
            .err()
            .unwrap()
            .to_string()
            .contains("only top-level Tasks")
    );
    assert!(NodeTarget::find(&targets, "missing").is_err());
}
