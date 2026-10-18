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
            {"id": "sink", "kind": "sink.number"}
        ],
        "edges": [{
            "from_node": "source",
            "from_output": "value",
            "to_node": "sink",
            "to_input": "input"
        }],
        "outputs": [{"name": "final", "node": "sink", "port": "result"}]
    })
}

fn validation_error(value: Value) -> WorkflowCompileError {
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    validate_definition(&definition, &registry).unwrap_err()
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
    let mut value = valid_definition();
    value["nodes"][0]["id"] = json!("   ");
    let error = validation_error(value);
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
    let mut value = valid_definition();
    value["edges"][0]["from_node"] = json!("missing-source");
    let error = validation_error(value);
    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownEdgeSource { .. }
    ));
    assert!(error.to_string().contains("missing-source"));

    let mut value = valid_definition();
    value["edges"][0]["to_node"] = json!("missing-target");
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::UnknownEdgeTarget { .. }
    ));

    let mut value = valid_definition();
    value["edges"][0]["from_output"] = json!("absent");
    let error = validation_error(value);
    assert!(matches!(
        &error,
        WorkflowCompileError::UnknownOutputPort { .. }
    ));
    assert!(error.to_string().contains("`source`.`absent`"));
    assert!(error.to_string().contains("`sink`.`input`"));

    let mut value = valid_definition();
    value["edges"][0]["to_input"] = json!("absent");
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

    let mut value = valid_definition();
    let duplicate = value["edges"][0].clone();
    value["edges"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        validation_error(value),
        WorkflowCompileError::DuplicateInputConnection { .. }
    ));

    let mut value = valid_definition();
    value["edges"] = json!([]);
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
    value["edges"] = json!([]);
    value["exclusive_inputs"] = json!([{
        "to_node":"sink", "to_input":"input", "sources":[
            {"from_node":"source", "from_output":"value"},
            {"from_node":"alternative", "from_output":"value"}
        ]
    }]);
    value
}

#[test]
fn exclusive_bindings_preserve_groups_and_validate_all_predecessors() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(exclusive_definition()).unwrap();
    let plan = mf_compiler::compile_definition(&definition, &registry).unwrap();
    assert_eq!(plan.execution_order.last().unwrap().as_str(), "sink");
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
    reordered.exclusive_inputs[0].sources.reverse();
    assert_eq!(
        plan,
        mf_compiler::compile_definition(&reordered, &registry).unwrap()
    );
    let nodes = mf_compiler::resolve_nodes(&definition, &registry).unwrap();
    let order = plan.execution_order.clone();
    mf_compiler::FlowBuilder::prepare_with_exclusive_inputs(
        nodes,
        vec![],
        definition.exclusive_inputs.clone(),
        order,
        definition.outputs.clone(),
    )
    .unwrap();
}

#[test]
fn exclusive_bindings_reject_conflicts_and_invalid_candidates() {
    for case in [
        "single",
        "duplicate",
        "edge",
        "group",
        "source",
        "target",
        "port",
        "type",
        "cycle",
    ] {
        let mut value = exclusive_definition();
        match case {
            "single" => {
                value["exclusive_inputs"][0]["sources"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "duplicate" => {
                value["exclusive_inputs"][0]["sources"][1] =
                    value["exclusive_inputs"][0]["sources"][0].clone();
            }
            "edge" => {
                value["edges"] = valid_definition()["edges"].clone();
            }
            "group" => {
                let group = value["exclusive_inputs"][0].clone();
                value["exclusive_inputs"]
                    .as_array_mut()
                    .unwrap()
                    .push(group);
            }
            "source" => value["exclusive_inputs"][0]["sources"][1]["from_node"] = json!("missing"),
            "target" => value["exclusive_inputs"][0]["to_node"] = json!("missing"),
            "port" => value["exclusive_inputs"][0]["sources"][1]["from_output"] = json!("missing"),
            "type" => value["nodes"][2]["kind"] = json!("source.string"),
            "cycle" => {
                value["exclusive_inputs"][0]["sources"][1] =
                    json!({"from_node":"sink", "from_output":"result"});
            }
            _ => unreachable!(),
        }
        let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
        if !matches!(case, "port" | "type") {
            assert!(
                mf_compiler::plan_definition(&definition).is_err(),
                "structural planning accepted {case}"
            );
        }
        assert!(
            validate_definition(&definition, &NodeRegistry::from_inventory().unwrap()).is_err(),
            "{case}"
        );
    }
}

#[test]
fn exclusive_syntax_is_strict_and_omitted_when_empty() {
    let ordinary: WorkflowDefinition = serde_json::from_value(valid_definition()).unwrap();
    assert!(
        serde_json::to_value(ordinary)
            .unwrap()
            .get("exclusive_inputs")
            .is_none()
    );
    let definition: WorkflowDefinition = serde_json::from_value(exclusive_definition()).unwrap();
    assert_eq!(
        definition,
        serde_json::from_value(serde_json::to_value(&definition).unwrap()).unwrap()
    );
    for field in ["group", "source"] {
        let mut value = exclusive_definition();
        if field == "group" {
            value["exclusive_inputs"][0]["policy"] = json!("first");
        } else {
            value["exclusive_inputs"][0]["sources"][0]["optional"] = json!(true);
        }
        assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
    }
}

#[test]
fn exclusive_candidates_cannot_cross_message_domains() {
    use mfn_core as _;
    let value = json!({
        "version":"2026-10-03", "dependencies":{}, "execution":{"mode":"stream"},
        "nodes":[
            {"id":"source", "kind":"source.number"},
            {"id":"batch", "kind":"builtin.batch", "config":{"max_items":2, "max_wait_ms":100}},
            {"id":"sink", "kind":"sink.any"}
        ],
        "edges":[{"from_node":"source", "from_output":"value", "to_node":"batch", "to_input":"item"}],
        "exclusive_inputs":[{"to_node":"sink", "to_input":"input", "sources":[
            {"from_node":"source", "from_output":"value"},
            {"from_node":"batch", "from_output":"items"}
        ]}]
    });
    let error = validation_error(value).to_string();
    assert!(error.contains("domain"), "{error}");
}
