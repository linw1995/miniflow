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
fn binds_a_frozen_child_interface_without_partitioning_its_flow() {
    use std::borrow::Cow;
    let flow = mf_runtime::Flow::from_plan(
        vec![],
        mf_runtime::FlowPlan {
            connections: Cow::Borrowed(&[]),
            dependencies: Cow::Borrowed(&[]),
            execution_order: Cow::Borrowed(&[]),
            outputs: Cow::Borrowed(&[]),
            execution_domains: mf_runtime::ExecutionDomains::from_parts(
                Cow::Borrowed(&[]),
                Cow::Borrowed(&[]),
            ),
        },
        Default::default(),
    );
    let interface = mf_runtime::SubgraphInterface {
        outputs: vec![PortSpec::new("compiled_output", ValueType::String, true)],
        stream_outputs: vec![PortSpec::new("compiled_stream", ValueType::String, false)],
        input_metadata: mf_runtime::SubgraphInputMetadata {
            flows: vec![mf_runtime::SubgraphFlowInputMetadata {
                nodes: vec!["compiled_component".into()],
                inputs: Default::default(),
            }],
        },
    };
    let body = mf_runtime::PreparedSubgraph::from_plan(flow, vec![], interface.clone());
    assert_eq!(body.outputs, interface.outputs);
    assert_eq!(body.stream_outputs, interface.stream_outputs);
    assert_eq!(body.input_metadata(), &interface.input_metadata);
}

#[test]
fn binds_proved_types_without_repeating_inference_or_executing_a_node() {
    struct NeverRun;
    impl mf_runtime::TaskNode for NeverRun {
        fn execute(
            &self,
            _: mf_runtime::Inputs,
            _: &mut mf_runtime::ExecutionContext,
        ) -> Result<mf_runtime::NodeResult, mf_runtime::NodeExecutionError> {
            panic!("binding must not execute providers");
        }
    }
    let prepare = || {
        let mut metadata = NodeMetadata::new(NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Any, true)],
        });
        // Binding consumes compiler evidence, rather than interpreting derivations again.
        metadata
            .output_derivations
            .push(OutputDerivation::forward_input(
                "value",
                "not-a-runtime-binding",
            ));
        mf_runtime::FlowNode::new(
            "source",
            mf_runtime::PreparedNode::from_parts(
                mf_runtime::NodeExecution::Task(Box::new(NeverRun)),
                metadata,
            ),
        )
    };
    let declaration = NodeContract::from(&prepare().metadata);
    let mut resolved = declaration.clone();
    resolved.ports.outputs[0].value_type = ValueType::Int64;
    let bound = mf_runtime::generated::bind(
        prepare(),
        &declaration,
        &resolved,
        mf_runtime::generated::ExecutorKind::Task,
        true,
    )
    .unwrap();
    assert_eq!(bound.metadata.ports.outputs[0].value_type, ValueType::Int64);
    for (kind, success) in [
        (mf_runtime::generated::ExecutorKind::Stream, true),
        (mf_runtime::generated::ExecutorKind::Task, false),
    ] {
        assert!(
            mf_runtime::generated::bind(prepare(), &declaration, &resolved, kind, success).is_err()
        );
    }
    let mut drift = declaration.clone();
    drift.ports.outputs[0].required = false;
    assert!(
        mf_runtime::generated::bind(
            prepare(),
            &drift,
            &resolved,
            mf_runtime::generated::ExecutorKind::Task,
            true
        )
        .is_err()
    );
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
