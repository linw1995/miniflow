use mf_runtime::{
    ContextReference, InputBinding, NodeContract, NodeMetadata, NodePorts, OutputDerivation,
    PortSpec, StdinRequirement, ValueType, verify_generated_metadata,
};

fn metadata() -> NodeMetadata {
    NodeMetadata {
        input_bindings: [(
            "value".into(),
            InputBinding {
                secret: true,
                ..Default::default()
            },
        )]
        .into(),
        context_references: vec![ContextReference::new("source.value", "condition")],
        stdin: Some(StdinRequirement::UnlessInput("value".into())),
        ..NodeMetadata::new(NodePorts {
            inputs: vec![PortSpec::new("value", ValueType::String, false)],
            outputs: vec![PortSpec::new("result", ValueType::String, true)],
        })
    }
}

#[test]
fn compares_execution_declarations_without_serializing_compiler_evidence() {
    let mut actual = metadata();
    let expected = NodeContract::from(&actual);
    actual
        .output_derivations
        .push(OutputDerivation::forward_input("result", "value"));
    verify_generated_metadata(&actual, &expected).unwrap();
    for field in [
        "ports",
        "required",
        "stream",
        "exclusive",
        "binding",
        "context",
        "stdin",
        "generation",
    ] {
        let mut changed = actual.clone();
        match field {
            "ports" => changed.ports.outputs[0].value_type = ValueType::Any,
            "required" => changed.ports.inputs[0].required = true,
            "stream" => {
                changed
                    .stream_outputs
                    .push(PortSpec::new("delta", ValueType::String, false))
            }
            "exclusive" => changed.exclusive_outputs.push("result".into()),
            "binding" => changed.input_bindings.clear(),
            "context" => changed.context_references.clear(),
            "stdin" => changed.stdin = None,
            "generation" => {
                changed.typed_generation = Some(mf_runtime::TypedGeneration {
                    constructor: mf_runtime::TypedConstructor {
                        package: "fixture",
                        path: &["task"],
                    },
                    inputs: vec![],
                    outputs: vec![],
                    context_free: false,
                })
            }
            _ => unreachable!(),
        }
        assert!(
            verify_generated_metadata(&changed, &expected).is_err(),
            "{field}"
        );
    }
}
