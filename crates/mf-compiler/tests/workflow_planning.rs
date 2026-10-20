use mf_compiler::{DefinitionId, WorkflowDefinition};
use mf_compiler::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs, PortSpec,
    TaskNode, ValueType, WorkflowCompileError, resolve_nodes, topological_order,
};
use serde_json::{Value, json};

struct NoopNode;

impl TaskNode for NoopNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok(Outputs::new().into())
    }
}

fn noop_factory(
    _config: Value,
    declared_ports: mf_runtime::NodePorts,
) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = NoopNode;
    let metadata = mf_runtime::NodeMetadata::new(declared_ports);
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

inventory::submit! {
    NodeRegistration { kind: "plan.source", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "plan.pass", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![PortSpec::new("input", ValueType::Number, true)], outputs: vec![PortSpec::new("value", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "plan.join", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![
            PortSpec::new("left", ValueType::Number, true),
            PortSpec::new("right", ValueType::Number, true),
        ], outputs: vec![PortSpec::new("result", ValueType::Number, true)] })) }
}

fn diamond_definition() -> Value {
    json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [
            {"id":"sink","kind":"plan.join"},
            {"id":"right","kind":"plan.pass"},
            {"id":"root","kind":"plan.source"},
            {"id":"left","kind":"plan.pass"}
        ],
        "edges": [
            {"from_node":"root","from_output":"value","to_node":"right","to_input":"input"},
            {"from_node":"right","from_output":"value","to_node":"sink","to_input":"right"},
            {"from_node":"root","from_output":"value","to_node":"left","to_input":"input"},
            {"from_node":"left","from_output":"value","to_node":"sink","to_input":"left"}
        ],
        "outputs": [{"name":"final","node":"sink","port":"result"}]
    })
}

#[test]
fn domain_snapshots_include_all_ancestor_ports_but_exclude_siblings() {
    let definition: WorkflowDefinition = serde_json::from_value(diamond_definition()).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let compiled = mf_compiler::compile_definition(&definition, &registry).unwrap();
    let flow = mf_compiler::instantiate_compiled(&compiled, &registry).unwrap();
    let visible: std::collections::BTreeMap<_, _> = flow
        .execution_domains()
        .domains()
        .iter()
        .map(|domain| {
            (
                flow.definition_node_id(&domain.nodes[0]).unwrap(),
                domain.snapshot_visibility.keys().collect::<Vec<_>>(),
            )
        })
        .collect();
    assert!(visible["root"].is_empty());
    assert_eq!(visible["left"], ["root.value"]);
    assert_eq!(visible["right"], ["root.value"]);
    assert_eq!(visible["sink"], ["left.value", "right.value", "root.value"]);
    #[cfg(feature = "codegen")]
    {
        let generated = compiled
            .generate_execution_plans(&registry)
            .unwrap()
            .rust_source;
        assert!(generated.contains("OutputVisibility::Borrowed"));
        assert!(generated.contains("\"root.value\""));
    }
}

#[test]
fn chooses_a_stable_order_when_multiple_nodes_are_ready() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let value = diamond_definition();
    let definition: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    let order = topological_order(&definition, &registry).unwrap();
    let expected: Vec<DefinitionId> = ["root", "left", "right", "sink"]
        .into_iter()
        .map(DefinitionId::from)
        .collect();

    assert_eq!(order, expected);
    assert!(
        mf_compiler::build_flow(
            resolve_nodes(&definition, &registry).unwrap(),
            definition.edges.clone(),
            order,
            definition.outputs.clone(),
        )
        .is_ok()
    );

    let mut reordered = value;
    reordered["nodes"].as_array_mut().unwrap().reverse();
    reordered["edges"].as_array_mut().unwrap().reverse();
    let reordered: WorkflowDefinition = serde_json::from_value(reordered).unwrap();
    assert_eq!(topological_order(&reordered, &registry).unwrap(), expected);
}

#[test]
fn reports_only_nodes_on_the_cycle() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [
            {"id":"a","kind":"plan.pass"},
            {"id":"b","kind":"plan.pass"},
            {"id":"0-tail","kind":"plan.pass"}
        ],
        "edges": [
            {"from_node":"a","from_output":"value","to_node":"b","to_input":"input"},
            {"from_node":"b","from_output":"value","to_node":"a","to_input":"input"},
            {"from_node":"b","from_output":"value","to_node":"0-tail","to_input":"input"}
        ]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();

    let error = topological_order(&definition, &registry).unwrap_err();
    let WorkflowCompileError::Cycle { path } = error else {
        panic!("expected a cycle error");
    };
    let nodes: Vec<&str> = path.nodes().iter().map(DefinitionId::as_str).collect();
    assert_eq!(nodes, ["a", "b", "a"]);
    assert_eq!(path.to_string(), "a -> b -> a");
}

#[test]
fn reports_self_loops() {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [{"id":"self","kind":"plan.pass"}],
        "edges": [{"from_node":"self","from_output":"value","to_node":"self","to_input":"input"}]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();

    let error = topological_order(&definition, &registry).unwrap_err();
    let WorkflowCompileError::Cycle { path } = error else {
        panic!("expected a cycle error");
    };
    assert_eq!(path.to_string(), "self -> self");
}
