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

fn candidate(source: &str) -> ExecutionDependency<'_> {
    ExecutionDependency {
        exclusive: true,
        ..dependency(Some("value"), source)
    }
}

#[test]
fn exclusive_resolution_preserves_payloads_and_metadata() {
    let mut target = node();
    target.metadata.ports.inputs[0].value_type = ValueType::Any;
    let state = ExecutionInputs::default();
    for value in [
        ValueRef::from(false),
        ValueRef::null(),
        ValueRef::from(json!({"token":"sensitive"})).into_secret(),
    ] {
        for selected in ["left", "right"] {
            let mut inputs = Inputs::new();
            let causes = state
                .resolve_node(
                    &target,
                    false,
                    [candidate("left"), candidate("right")],
                    |source| {
                        Ok(Some(if source.source_node == selected {
                            ContextValue::Value(&value)
                        } else {
                            ContextValue::Skipped
                        }))
                    },
                    &mut inputs,
                )
                .unwrap();
            assert!(causes.is_empty());
            assert!(inputs["value"].ptr_eq(&value));
            assert_eq!(inputs["value"].redacted(), value.redacted());
        }
    }
}

#[test]
fn exclusive_errors_precede_skips_and_input_type_checks() {
    let state = ExecutionInputs::default();
    let bad = ValueRef::from("sentinel-secret").into_secret();
    for reversed in [false, true] {
        for case in ["one", "none", "ambiguous", "missing", "missing_ambiguous"] {
            let mut dependencies = vec![
                candidate("left"),
                candidate("right"),
                candidate("third"),
                dependency(None, "gate"),
            ];
            if reversed {
                dependencies.reverse();
            }
            let mut inputs = Inputs::new();
            let result = state.resolve_node(
                &node(),
                false,
                dependencies,
                |source| {
                    if source.source_node == "gate" {
                        return Ok(Some(ContextValue::Skipped));
                    }
                    if source.source_node == "third" && case.starts_with("missing") {
                        return mf_runtime::NodeExecutionFailedSnafu {
                            message: "missing third.value",
                        }
                        .fail();
                    }
                    let produced = source.source_node == "left" && case != "none"
                        || source.source_node == "right" && case.contains("ambiguous");
                    Ok(Some(if produced {
                        ContextValue::Value(&bad)
                    } else {
                        ContextValue::Skipped
                    }))
                },
                &mut inputs,
            );
            match case {
                "one" => {
                    let causes = result.unwrap();
                    assert_eq!(causes.len(), 1);
                    assert_eq!(causes.first().unwrap().source_node, "gate");
                }
                "none" => assert_eq!(result.unwrap().len(), 4),
                "ambiguous" => {
                    let error = result.unwrap_err();
                    assert!(matches!(
                        error,
                        mf_runtime::WorkflowRunError::AmbiguousInput { .. }
                    ));
                    assert!(!format!("{error:?}").contains("sentinel-secret"));
                }
                _ => assert!(matches!(
                    result.unwrap_err(),
                    mf_runtime::WorkflowRunError::Dependency { .. }
                )),
            }
        }
    }
    let mut inputs = Inputs::new();
    let error = state
        .resolve_node(
            &node(),
            false,
            [candidate("left"), candidate("right")],
            |source| {
                Ok(Some(if source.source_node == "left" {
                    ContextValue::Value(&bad)
                } else {
                    ContextValue::Skipped
                }))
            },
            &mut inputs,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        mf_runtime::WorkflowRunError::InputType { .. }
    ));
}

#[test]
fn all_skipped_optional_group_remains_a_skip() {
    let mut target = node();
    target.metadata.ports.inputs[0].required = false;
    let mut inputs = Inputs::new();
    let causes = ExecutionInputs::default()
        .resolve_node(
            &target,
            false,
            [candidate("left"), candidate("right")],
            |_| Ok(Some(ContextValue::Skipped)),
            &mut inputs,
        )
        .unwrap();
    assert_eq!(causes.len(), 2);
    assert!(inputs.is_empty());
}
