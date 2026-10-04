use mf_compiler::{
    CompiledWorkflow, Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry,
    Outputs, PortSpec, TaskNode, ValueType, WorkflowCompileError, compile_definition,
    deserialize_config, instantiate_compiled,
};
use mf_compiler::{DefinitionId, WorkflowDefinition};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct ConstantConfig {
    value: i64,
}

struct ConstantNode {
    value: i64,
}

impl TaskNode for ConstantNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok((Outputs::from([("value".to_owned(), json!(self.value).into())])).into())
    }
}

struct IncrementNode;

impl TaskNode for IncrementNode {
    fn execute(
        &self,
        inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        let value = inputs["input"].as_i64().unwrap() + 1;
        Ok((Outputs::from([("value".to_owned(), json!(value).into())])).into())
    }
}

fn constant_factory(config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let config: ConstantConfig = deserialize_config(config)?;
    let node = ConstantNode {
        value: config.value,
    };
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![],
        outputs: vec![PortSpec::new("value", ValueType::Number, true)],
    });
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

fn increment_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = IncrementNode;
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![PortSpec::new("input", ValueType::Number, true)],
        outputs: vec![PortSpec::new("value", ValueType::Number, true)],
    });
    Ok(mf_runtime::PreparedNode::new(node, metadata))
}

inventory::submit! {
    NodeRegistration { kind: "example.constant", factory: mf_runtime::NodeFactory::Plain(constant_factory) }
}

inventory::submit! {
    NodeRegistration { kind: "example.increment", factory: mf_runtime::NodeFactory::Plain(increment_factory) }
}

fn definition() -> WorkflowDefinition {
    WorkflowDefinition::from_json(
        r#"{
            "version": "2026-09-26", "dependencies": {},
            "nodes":[
                {"id":"increment-two","kind":"example.increment"},
                {"id":"increment","kind":"example.increment"},
                {"id":"source","kind":"example.constant","config":{"value":41}}
            ],
            "edges":[
                {"from_node":"source","from_output":"value","to_node":"increment-two","to_input":"input"},
                {"from_node":"source","from_output":"value","to_node":"increment","to_input":"input"}
            ],
            "outputs":[
                {"name":"other","node":"increment-two","port":"value"},
                {"name":"answer","node":"increment","port":"value"}
            ]
        }"#,
    )
    .unwrap()
}

#[test]
fn generated_plan_round_trips_and_preserves_definition_semantics() {
    let definition = definition();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let generated = plan.generate_artifacts().unwrap();
    let reparsed = CompiledWorkflow::from_json(&generated.plan_json).unwrap();

    assert_eq!(reparsed, plan);
    assert!(generated.rust_source.contains("pub fn run_workflow("));
    assert!(generated.rust_source.contains("mf_runtime::execute_node"));
    assert!(generated.rust_source.contains("state.select_output"));
    assert!(!generated.rust_source.contains("Flow::new"));
    assert!(!generated.rust_source.contains("WORKFLOW_PLAN_JSON"));

    let original_nodes: BTreeMap<DefinitionId, (String, Value)> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), (node.kind.clone(), node.config.clone())))
        .collect();
    let generated_nodes: BTreeMap<DefinitionId, (String, Value)> = reparsed
        .definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), (node.kind.clone(), node.config.clone())))
        .collect();
    assert_eq!(generated_nodes, original_nodes);

    let original_edges: BTreeSet<_> = definition
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from_node.clone(),
                edge.from_output.clone(),
                edge.to_node.clone(),
                edge.to_input.clone(),
            )
        })
        .collect();
    let generated_edges: BTreeSet<_> = reparsed
        .definition
        .edges
        .iter()
        .map(|edge| {
            (
                edge.from_node.clone(),
                edge.from_output.clone(),
                edge.to_node.clone(),
                edge.to_input.clone(),
            )
        })
        .collect();
    assert_eq!(generated_edges, original_edges);

    let original_outputs: BTreeSet<_> = definition
        .outputs
        .iter()
        .map(|output| {
            (
                output.name.clone(),
                output.node.clone(),
                output.port.clone(),
            )
        })
        .collect();
    let generated_outputs: BTreeSet<_> = reparsed
        .definition
        .outputs
        .iter()
        .map(|output| {
            (
                output.name.clone(),
                output.node.clone(),
                output.port.clone(),
            )
        })
        .collect();
    assert_eq!(generated_outputs, original_outputs);

    let flow = instantiate_compiled(&reparsed, &registry).unwrap();
    let outputs = flow.execute().unwrap();
    assert_eq!(outputs["answer"], json!(42));
    assert_eq!(outputs["other"], json!(42));

    let mut reordered = definition;
    reordered.nodes.reverse();
    reordered.edges.reverse();
    reordered.outputs.reverse();
    let reordered_plan = compile_definition(&reordered, &registry).unwrap();
    let reordered_generated = reordered_plan.generate_artifacts().unwrap();
    assert_eq!(reordered_generated.plan_json, generated.plan_json);
    assert_eq!(reordered_generated.rust_source, generated.rust_source);
}

#[test]
fn rejects_a_plan_with_a_modified_execution_order() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&definition(), &registry).unwrap();
    plan.execution_order.reverse();

    let error = instantiate_compiled(&plan, &registry).err().unwrap();
    assert!(matches!(error, WorkflowCompileError::NonCanonicalPlanOrder));
}

#[test]
fn code_generation_escapes_definition_strings() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [{
            "id": "source\"quoted\n",
            "kind": "example.constant",
            "config": {"value": 3, "label": "quoted\"value\nnext line"}
        }],
        "outputs": [{
            "name": "answer\"quoted",
            "node": "source\"quoted\n",
            "port": "value"
        }]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let artifacts = compile_definition(&definition, &registry)
        .unwrap()
        .generate_artifacts()
        .unwrap();

    syn::parse_file(&artifacts.rust_source).unwrap();
    assert!(artifacts.rust_source.contains("source\\\"quoted\\n"));
    assert!(artifacts.rust_source.contains("answer\\\"quoted"));
}
