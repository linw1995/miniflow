extern crate mfn_core as _;
use mf_compiler::WorkflowDefinition;
use mf_compiler::{
    Inputs, NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, Outputs, PortSpec,
    TaskNode, TypeDepthError, ValueType, WorkflowCompileError, validate_definition,
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

struct DeepTypeNode;

impl TaskNode for DeepTypeNode {
    fn execute(
        &self,
        _inputs: Inputs,
        _ctx: &mut mf_runtime::ExecutionContext,
    ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
        Ok(Outputs::new().into())
    }
}
impl DeepTypeNode {
    fn ports(&self) -> mf_compiler::NodePorts {
        let mut value_type = ValueType::Int64;
        for _ in 0..ValueType::MAX_DEPTH {
            value_type = ValueType::List(Box::new(value_type));
        }
        mf_compiler::NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::owned("value", value_type, true)],
        }
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

fn deep_type_factory(_config: Value) -> Result<mf_runtime::PreparedNode, NodeBuildError> {
    let node = DeepTypeNode;
    let metadata = mf_runtime::NodeMetadata::new(node.ports());
    Ok(mf_runtime::PreparedNode::from_parts(
        mf_runtime::NodeExecution::Task(Box::new(node)),
        metadata,
    ))
}

inventory::submit! {
    NodeRegistration { kind: "source.deep_type", factory: mf_runtime::NodeFactory::Plain(deep_type_factory) }
}

inventory::submit! {
    NodeRegistration { kind: "source.number", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "source.string", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::String, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "source.any", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![], outputs: vec![PortSpec::new("value", ValueType::Any, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "sink.number", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![PortSpec::new("input", ValueType::Number, true)], outputs: vec![PortSpec::new("result", ValueType::Number, true)] })) }
}

inventory::submit! {
    NodeRegistration { kind: "sink.any", factory: mf_runtime::NodeFactory::Plain(|config| noop_factory(config, mf_runtime::NodePorts { inputs: vec![PortSpec::new("input", ValueType::Any, true)], outputs: vec![PortSpec::new("result", ValueType::Any, true)] })) }
}

fn valid_definition() -> Value {
    json!({
        "version": "2026-09-26", "dependencies": {},
        "nodes": [
            {"id": "source", "kind": "source.number"},
            {"id": "sink", "kind": "sink.number", "inputs":{"input":{"$ref":"source.value"}}}
        ],
        "outputs": [{"name": "final", "node": "sink", "port": "result"}]
    })
}

fn validation_error(value: Value) -> WorkflowCompileError {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    invalid_graph(&definition)
}

fn invalid_graph(definition: &WorkflowDefinition) -> WorkflowCompileError {
    validate_definition(definition, &NodeRegistry::from_inventory().unwrap()).unwrap_err()
}

#[test]
fn accepts_valid_connections_and_specific_values_into_any_inputs() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    validate_definition(&definition, &registry).unwrap();

    let mut value = valid_definition();
    value["nodes"][1]["kind"] = json!("sink.any");
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    validate_definition(&definition, &registry).unwrap();
}

#[test]
fn rejects_blank_and_duplicate_node_ids() {
    let mut graph: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    graph.nodes[0].id = "   ".into();
    let error = invalid_graph(&graph);
    assert!(matches!(
        &error,
        WorkflowCompileError::InvalidNodeId { position: 1 }
    ));
    assert!(error.to_string().contains("position 1"));

    let mut value = valid_definition();
    value["nodes"][1]["id"] = json!("source");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::DuplicateNodeId { .. }
    ));
}

#[test]
fn rejects_unknown_edge_endpoints_and_ports() {
    let graph: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    let mut invalid = graph.clone();
    invalid.edges[0].from_node = "missing-source".into();
    let error = invalid_graph(&invalid);
    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownEdgeSource { .. }
    ));
    assert!(error.to_string().contains("missing-source"));

    let mut invalid = graph;
    invalid.edges[0].to_node = "missing-target".into();
    assert!(matches!(
        invalid_graph(&invalid),
        WorkflowCompileError::UnknownEdgeTarget { .. }
    ));

    let mut value = valid_definition();
    value["nodes"][1]["inputs"]["input"] = json!({"$ref":"source.absent"});
    let error = validation_error(value);
    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownOutputPort { .. }
    ));
    assert!(error.to_string().contains("`source`.`absent`"));
    assert!(error.to_string().contains("`sink`.`input`"));

    let mut value = valid_definition();
    value["nodes"][1]["inputs"] = json!({"absent":{"$ref":"source.value"}});
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownInputPort { .. }
    ));
}

#[test]
fn rejects_incompatible_or_ambiguous_inputs() {
    let mut value = valid_definition();
    value["nodes"][0]["kind"] = json!("source.string");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::IncompatiblePortTypes { .. }
    ));

    let mut value = valid_definition();
    value["nodes"][0]["kind"] = json!("source.any");
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    validate_definition(&definition, &NodeRegistry::from_inventory().unwrap()).unwrap();

    let mut graph: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    graph.edges.push(graph.edges[0].clone());
    assert!(matches!(
        invalid_graph(&graph),
        WorkflowCompileError::DuplicateInputConnection { .. }
    ));

    let mut value = valid_definition();
    value["nodes"][1].as_object_mut().unwrap().remove("inputs");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::MissingRequiredInput { .. }
    ));
}

#[test]
fn rejects_invalid_selected_outputs() {
    let mut value = valid_definition();
    value["outputs"][0]["node"] = json!("missing");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownWorkflowOutputNode { .. }
    ));

    let mut value = valid_definition();
    value["outputs"][0]["port"] = json!("missing");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownWorkflowOutputPort { .. }
    ));

    let mut value = valid_definition();
    let duplicate = value["outputs"][0].clone();
    value["outputs"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::DuplicateWorkflowOutputName { .. }
    ));
}

#[test]
fn rejects_excessively_nested_instance_port_types() {
    let mut value = valid_definition();
    value["nodes"][0]["kind"] = json!("source.deep_type");
    let error = validation_error(value);
    assert!(matches!(error, WorkflowCompileError::PortTypeDepth { .. }));
    let source = std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<TypeDepthError>()
        .unwrap();
    assert_eq!(source.depth, ValueType::MAX_DEPTH + 1);
    assert_eq!(source.maximum, ValueType::MAX_DEPTH);
    let message = error.to_string();
    assert!(message.contains("source"));
    assert!(message.contains("output port `value`"));
    assert!(message.contains("type nesting depth"));
}

fn exclusive_definition() -> Value {
    let mut value = valid_definition();
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"alternative", "kind":"source.number"}));
    value["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"condition", "kind":"builtin.constant", "config":{"value":true}}),
        json!({"id":"route", "kind":"builtin.if_else", "config":{"branches":[{"id":"first", "condition":{"source":{"output":"condition.value", "path":""}, "operator":"eq", "value":true}}]}})
    ]);
    value["edges"] = json!([
        {"from_node":"condition", "from_output":"value", "to_node":"route"},
        {"from_node":"route", "from_output":"first", "to_node":"source"},
        {"from_node":"route", "from_output":"else", "to_node":"alternative"}
    ]);
    value["nodes"][1]["inputs"]["input"] =
        json!({"exclusive":[{"$ref":"source.value"}, {"$ref":"alternative.value"}]});
    value
}

#[test]
fn exclusive_bindings_preserve_groups_and_validate_all_predecessors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(exclusive_definition()).unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    assert_eq!(plan.graph.execution_order.last().unwrap().as_str(), "sink");
    let flow = mf_compiler::instantiate_compiled(&plan, &registry).unwrap();
    assert_eq!(flow.connections().len(), 2);
    assert!(flow.connections().iter().all(|edge| edge.exclusive));
    assert!(
        flow.plan()
            .dependencies
            .last()
            .unwrap()
            .iter()
            .all(|dependency| dependency.exclusive)
    );
    let mut reordered = definition.clone();
    reordered.edges.reverse();
    assert_eq!(
        plan,
        mf_compiler::compile_definition(&reordered, &registry).unwrap()
    );
    let nodes = mf_compiler::resolve_nodes(&definition, &registry).unwrap();
    mf_compiler::FlowBuilder::prepare(
        nodes,
        definition.edges.clone(),
        plan.graph.execution_order.clone(),
        definition.outputs.clone(),
    )
    .unwrap();
}

#[test]
fn exclusive_bindings_reject_conflicts_and_invalid_candidates() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let valid: WorkflowDefinition = serde_json::from_value(exclusive_definition()).unwrap();
    let order = mf_compiler::plan_definition(&valid)
        .unwrap()
        .graph
        .execution_order;
    for case in [
        "single",
        "duplicate",
        "mixed",
        "mixed_reverse",
        "source",
        "target",
        "port",
        "type",
        "cycle",
    ] {
        let mut definition = valid.clone();
        match case {
            "single" => {
                definition.edges.pop();
            }
            "duplicate" => definition.edges[1] = definition.edges[0].clone(),
            "mixed" | "mixed_reverse" => {
                let mut ordinary = definition.edges[0].clone();
                ordinary.exclusive = false;
                if case == "mixed_reverse" {
                    definition.edges.insert(0, ordinary);
                } else {
                    definition.edges.push(ordinary);
                }
            }
            "source" => definition.edges[1].from_node = "missing".into(),
            "target" => {
                for edge in &mut definition.edges {
                    edge.to_node = "missing".into();
                }
            }
            "port" => definition.edges[1].from_output = "missing".into(),
            "type" => definition.nodes[2].kind = "source.string".into(),
            "cycle" => {
                definition.edges[1].from_node = "sink".into();
                definition.edges[1].from_output = "result".into();
            }
            _ => unreachable!(),
        }
        if !matches!(case, "port" | "type") {
            assert!(
                mf_compiler::plan_definition(&definition).is_err(),
                "structural planning accepted {case}"
            );
        }
        assert!(
            validate_definition(&definition, &registry).is_err(),
            "{case}"
        );
        if matches!(case, "single" | "duplicate" | "mixed" | "mixed_reverse") {
            let nodes = mf_compiler::resolve_nodes(&valid, &registry).unwrap();
            assert!(
                mf_compiler::FlowBuilder::prepare(
                    nodes,
                    definition.edges.clone(),
                    order.clone(),
                    valid.outputs.clone()
                )
                .is_err(),
                "programmatic builder accepted {case}"
            );
        }
    }
}

#[test]
fn input_bindings_round_trip_and_reject_invalid_exclusive_forms() {
    let ordinary: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    assert!(!ordinary.edges[0].exclusive);
    assert_eq!(
        json!(ordinary)["nodes"][1]["inputs"]["input"],
        json!({"$ref":"source.value"})
    );
    let group: WorkflowDefinition = serde_json::from_value(exclusive_definition()).unwrap();
    assert_eq!(group, serde_json::from_value(json!(group)).unwrap());
    for binding in [
        json!({"exclusive":false}),
        json!({"exclusive":null}),
        json!({"exclusive":"true"}),
        json!({"$ref":"source.value", "policy":"first"}),
    ] {
        let mut invalid = exclusive_definition();
        invalid["nodes"][1]["inputs"]["input"] = binding;
        assert!(serde_json::from_value::<WorkflowDefinition>(invalid).is_err());
    }
}

#[test]
fn exclusive_candidates_cannot_cross_message_domains() {
    use mfn_core as _;
    let value = json!({
        "version":"2026-10-03", "dependencies":{}, "execution":{},
        "nodes":[
            {"id":"source", "kind":"source.number"},
            {"id":"batch", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}, "inputs":{"item":{"$ref":"source.value"}}},
            {"id":"sink", "kind":"sink.any", "inputs":{"input":{"exclusive":[{"$ref":"source.value"}, {"$ref":"batch.items"}]}}}
        ]
    });
    let error = validation_error(value).to_string();
    assert!(error.contains("not proven mutually exclusive"), "{error}");
}
