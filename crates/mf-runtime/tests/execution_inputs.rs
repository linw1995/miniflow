use mf_runtime::{
    ContextValue, ExecutionDependency, ExecutionInputs, FlowNode, Inputs, NodeMetadata, NodePorts,
    PortSpec, StdinRequirement, ValueRef, ValueType, WorkflowArguments, WorkflowInputSchema,
};
use serde_json::json;

fn node() -> FlowNode<()> {
    FlowNode {
        definition_id: "target".into(),
        node: (),
        metadata: NodeMetadata::new(NodePorts {
            inputs: vec![PortSpec::new("value", ValueType::Int64, true)],
            outputs: vec![],
        }),
    }
}

fn schema() -> WorkflowInputSchema {
    serde_json::from_value(json!({
        "inputs": {"target": {"value": {
            "type": "int", "required": true, "binding": {"env": "VALUE", "default": 1}
        }}},
        "stdin": {}
    }))
    .unwrap()
}

fn dependency<'a>(input: Option<&'a str>, source: &'a str) -> ExecutionDependency<'a> {
    ExecutionDependency {
        exclusive: false,
        input,
        source_node: source,
        source_output: "value",
    }
}

#[test]
fn binding_refreshes_sources_and_invalidates_state_on_every_failure() {
    let node = node();
    let mut schema = schema();
    let mut inputs = ExecutionInputs::default();
    for value in [2, 3] {
        inputs
            .bind(&schema, |_| Some(value.to_string().into()), false)
            .unwrap();
        assert_eq!(inputs.initial(&node, false)["value"], json!(value));
    }
    inputs.bind(&schema, |_| None, false).unwrap();
    assert_eq!(inputs.initial(&node, false)["value"], json!(1));
    assert!(
        inputs
            .bind(&schema, |_| Some("invalid".into()), false)
            .is_err()
    );
    assert!(inputs.initial(&node, false).is_empty());

    schema
        .stdin
        .insert("target".into(), StdinRequirement::Always);
    inputs.bind(&schema, |_| None, true).unwrap();
    assert_eq!(inputs.stdin_owner(), Some("target"));
    assert!(inputs.bind(&schema, |_| None, false).is_err());
    assert!(inputs.initial(&node, false).is_empty());
    assert_eq!(inputs.stdin_owner(), None);

    inputs.set_arguments(WorkflowArguments::try_from(json!({"target.value": 9})).unwrap());
    inputs
        .bind(
            &schema,
            |_| panic!("explicit arguments override environment"),
            true,
        )
        .unwrap();
    assert_eq!(inputs.initial(&node, false)["value"], json!(9));
    inputs.set_arguments(WorkflowArguments::default());
    assert!(inputs.initial(&node, false).is_empty());
    assert_eq!(inputs.stdin_owner(), None);
}

#[test]
fn node_resolution_combines_startup_values_and_dependencies_without_copying_payloads() {
    let mut state = ExecutionInputs::default();
    state.bind(&schema(), |_| None, false).unwrap();
    let value = ValueRef::from(42).into_secret();
    let mut inputs = Inputs::new();
    let causes = state
        .resolve_node(
            &node(),
            false,
            [
                dependency(None, "control"),
                dependency(Some("value"), "source"),
            ],
            |_| Ok(Some(ContextValue::Value(&value))),
            &mut inputs,
        )
        .unwrap();
    assert!(causes.is_empty());
    assert_eq!(inputs.len(), 1);
    assert!(inputs["value"].ptr_eq(&value));
    assert!(inputs["value"].is_secret());
}
