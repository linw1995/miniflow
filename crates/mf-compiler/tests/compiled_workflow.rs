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
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

fn increment_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = IncrementNode;
    let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
        inputs: vec![PortSpec::new("input", ValueType::Number, true)],
        outputs: vec![PortSpec::new("value", ValueType::Number, true)],
    });
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
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
    let encoded: Value = serde_json::from_str(&generated.plan_json).unwrap();
    assert_eq!(encoded.as_object().unwrap().len(), 1);
    assert!(encoded.get("graph").is_some());
    assert!(encoded["graph"].get("definition").is_some());
    assert!(encoded["graph"].get("execution_order").is_some());
    assert!(encoded["graph"].get("parameter_types").is_none());
    assert!(encoded["graph"].get("loop_orders").is_none());
    assert!(generated.rust_source.contains("pub fn run_workflow("));

    let original_nodes: BTreeMap<DefinitionId, (String, Value)> = definition
        .nodes
        .iter()
        .map(|node| (node.id.clone(), (node.kind.clone(), node.config.clone())))
        .collect();
    let generated_nodes: BTreeMap<DefinitionId, (String, Value)> = reparsed
        .graph
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
        .graph
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
        .graph
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
fn plain_configuration_stays_opaque_in_saved_graphs_and_offline_descriptions() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut definition = definition();
    let opaque = json!({
        "definition": {"version": "not-a-workflow", "nodes": []},
        "path": "missing-child.json",
        "body": {"builtin": "unlinked.child"}
    });
    definition.nodes[2].config["ordinary"] = opaque.clone();
    let plan = compile_definition(&definition, &registry).unwrap();
    assert!(plan.graph.children.is_empty());
    let saved = CompiledWorkflow::from_json(&plan.to_json().unwrap()).unwrap();
    let source = saved
        .graph
        .definition
        .nodes
        .iter()
        .find(|node| node.id.as_str() == "source")
        .unwrap();
    assert_eq!(source.config["ordinary"], opaque);
    assert!(
        mf_compiler::describe_compiled(&saved)
            .unwrap()
            .loop_bodies
            .is_empty()
    );
    assert_eq!(
        instantiate_compiled(&saved, &registry)
            .unwrap()
            .execute()
            .unwrap()["answer"],
        json!(42)
    );
}

#[test]
fn saved_plans_require_explicit_graph_and_reject_parameter_metadata() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition(), &registry).unwrap();
    let encoded = serde_json::to_value(&plan).unwrap();
    assert!(CompiledWorkflow::from_json(&encoded["graph"].to_string()).is_err());
    for field in ["parameter_types", "loop_orders"] {
        let mut legacy = encoded.clone();
        legacy[field] = json!({});
        assert!(CompiledWorkflow::from_json(&legacy.to_string()).is_err());
        let mut legacy = encoded.clone();
        legacy["graph"][field] = json!({});
        assert!(CompiledWorkflow::from_json(&legacy.to_string()).is_err());
    }
}

#[test]
fn rejects_a_plan_with_a_modified_execution_order() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let mut plan = compile_definition(&definition(), &registry).unwrap();
    plan.graph.execution_order.reverse();

    let error = instantiate_compiled(&plan, &registry).err().unwrap();
    assert!(matches!(error, WorkflowCompileError::NonCanonicalPlanOrder));
    #[cfg(feature = "codegen")]
    assert!(matches!(
        plan.generate_embedded_artifacts().unwrap_err(),
        mf_compiler::PlanError::InvalidExecutionOrder
    ));
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
    let plan = compile_definition(&definition, &registry).unwrap();
    let artifacts = plan.generate_artifacts().unwrap();
    let layouts = plan.generate_execution_plans(&registry).unwrap();

    syn::parse_file(&artifacts.rust_source).unwrap();
    syn::parse_file(&layouts.rust_source).unwrap();
    assert!(artifacts.rust_source.contains("source\\\"quoted\\n"));
    assert!(layouts.rust_source.contains("answer\\\"quoted"));
}
