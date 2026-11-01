#![cfg(feature = "codegen")]
extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use mf_runtime::{Inputs, NodeMetadata, NodeTarget, Outputs};
use serde_json::{Value, json};
use snafu::ResultExt;
use std::sync::atomic::{AtomicUsize, Ordering};

static PREPARATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    let _guard = PREPARATION_LOCK.lock().unwrap();
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
        NodeTarget::find(&targets, "route").unwrap().context["source.value"],
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
                    output_keys: Default::default(),
                }
            })
            .collect();
        let changed =
            mf_runtime::Flow::from_plan(nodes, flow.plan().clone(), flow.input_schema().clone());
        let error = copy.bind_flow(&changed).err().unwrap();
        assert!(error.to_string().contains("copy") && error.to_string().contains("drift"));
    }

    let route = NodeTarget::find(&targets, "route").unwrap();
    let mut altered_reference = route.clone();
    altered_reference
        .context
        .insert("source.value".into(), mf_runtime::ValueType::String);
    assert!(
        altered_reference
            .bind_flow(&flow)
            .err()
            .unwrap()
            .to_string()
            .contains("referenced output")
    );
    let nodes = flow
        .nodes()
        .iter()
        .map(|node| {
            let mut metadata = node.metadata.clone();
            for reference in &mut metadata.context_references {
                reference.label = "reworded diagnostic".into();
            }
            mf_runtime::TaskFlowNode {
                definition_id: node.definition_id.clone(),
                metadata,
                node: Box::new(Probe),
                output_keys: Default::default(),
            }
        })
        .collect();
    let reworded =
        mf_runtime::Flow::from_plan(nodes, flow.plan().clone(), flow.input_schema().clone());
    assert!(route.bind_flow(&reworded).is_ok());

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

#[test]
fn injected_flow_bodies_follow_their_owning_node_target() {
    let _guard = PREPARATION_LOCK.lock().unwrap();
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/chat-calculator-flow.json");
    let resolved = mf_compiler::load_workflow(&example).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let description = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(description.injected_bodies.len(), 1);
    let artifacts = plan.generate_execution_plans(&registry).unwrap();
    let targets: Vec<NodeTarget> = serde_json::from_str(&artifacts.node_targets_json).unwrap();
    let owner = description.injected_bodies[0]
        .first()
        .expect("injected body has a root")
        .clone();
    assert!(targets.iter().any(|target| target.id == owner));
    for target in &targets {
        if target.id == owner {
            assert_eq!(
                target.description.injected_bodies,
                description.injected_bodies
            );
        } else {
            assert!(
                target.description.injected_bodies.is_empty(),
                "{}",
                target.id
            );
        }
    }
}

#[test]
fn public_target_preparation_rejects_incomplete_or_unresolved_metadata() {
    let _guard = PREPARATION_LOCK.lock().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let mut nodes = mf_compiler::resolve_nodes(&plan.definition, &registry).unwrap();
    assert!(matches!(
        plan.node_targets(&nodes[..1]),
        Err(mf_compiler::PlanError::InvalidExecutionOrder)
    ));
    let route = nodes
        .iter_mut()
        .find(|node| node.definition_id.as_str() == "route")
        .unwrap();
    route.metadata.context_references =
        vec![mf_runtime::ContextReference::new("absent.value", "unknown")];
    assert!(matches!(
        plan.node_targets(&nodes),
        Err(mf_compiler::PlanError::UnknownTargetReference { .. })
    ));
}
