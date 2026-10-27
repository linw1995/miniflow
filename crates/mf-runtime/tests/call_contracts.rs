use mf_runtime::{
    CallArguments, CallBinding, CallBindingId, CallContractError, CallId, CallInputDefaults,
    CallOutcome, CallPortMapping, CallRequest, NodeBuildError, NodeExecutionError, NodeId,
    NodePluginFailedSnafu, NodeValues, SkipCause, ValueRef, WorkflowRunError,
};
use snafu::IntoError;
use std::error::Error;

fn mapping(output: &str, input: &str) -> CallPortMapping {
    CallPortMapping {
        output: output.into(),
        input: input.into(),
    }
}

fn request(binding: usize, invocation: u64, arguments: CallArguments) -> CallRequest {
    CallRequest {
        id: CallId {
            binding: CallBindingId::new(binding),
            invocation,
        },
        arguments,
    }
}

#[test]
fn explicit_groups_do_not_enable_node_level_defaults() {
    let ports = vec![mapping("request.query", "query"), mapping("limit", "limit")];
    let explicit = CallBinding::new(
        "search",
        NodeId::new(3),
        ports.clone(),
        CallInputDefaults::default(),
    )
    .unwrap();
    assert_eq!(explicit.name(), "search");
    assert_eq!(explicit.target(), NodeId::new(3));
    assert_eq!(explicit.ports(), ports);
    assert_eq!(explicit.defaults(), CallInputDefaults::ExplicitOnly);

    let defaulted = CallBinding::new(
        "search",
        NodeId::new(3),
        ports.clone(),
        CallInputDefaults::RemainingInputs,
    )
    .unwrap();
    assert_eq!(defaulted.ports(), ports);
    assert_eq!(defaulted.defaults(), CallInputDefaults::RemainingInputs);
}

#[test]
fn groups_reject_duplicate_destinations_but_allow_shared_source_outputs() {
    for ports in [
        vec![mapping("first", "query"), mapping("second", "query")],
        vec![mapping("query", "query"), mapping("query", "query")],
    ] {
        let error = CallBinding::new(
            "search",
            NodeId::new(0),
            ports,
            CallInputDefaults::ExplicitOnly,
        )
        .unwrap_err();
        assert!(matches!(&error, CallContractError::DuplicateInput { input } if input == "query"));
        assert!(error.to_string().contains("input `query`"));
    }
    let ports = vec![mapping("value", "left"), mapping("value", "right")];
    assert!(
        CallBinding::new(
            "compare",
            NodeId::new(0),
            ports,
            CallInputDefaults::ExplicitOnly
        )
        .is_ok()
    );
}

#[test]
fn declarations_validate_names_without_interpreting_port_paths() {
    for (name, ports, field) in [
        (" ", vec![], "binding"),
        ("search", vec![mapping("", "query")], "output"),
        ("search", vec![mapping("query", "")], "input"),
    ] {
        let error = CallBinding::new(name, NodeId::new(0), ports, CallInputDefaults::ExplicitOnly)
            .unwrap_err();
        assert!(
            matches!(&error, CallContractError::EmptyName { field: actual } if *actual == field)
        );
        assert!(error.to_string().contains(field));
    }
    let ports = vec![mapping("request.a/~", "input.a/~")];
    let binding = CallBinding::new(
        "exact.names",
        NodeId::new(0),
        ports.clone(),
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap();
    assert_eq!(binding.ports(), ports);
}

#[test]
fn atomic_arguments_preserve_namespaces_sharing_and_presence() {
    let secret = ValueRef::from("private query").into_secret();
    let arguments = CallArguments {
        outputs: NodeValues::from([
            ("query".into(), secret.clone()),
            ("nullable".into(), ValueRef::null()),
        ]),
        defaults: NodeValues::from([("query".into(), "default target input".into())]),
    };
    let call = request(2, 10, arguments.clone());
    assert!(call.arguments.outputs["query"].ptr_eq(&secret));
    assert!(call.arguments.outputs["query"].is_secret());
    assert!(call.arguments.outputs["nullable"].is_null());
    assert!(!call.arguments.outputs.contains_key("limit"));
    assert_eq!(
        call.arguments.defaults["query"].as_str(),
        Some("default target input")
    );
    assert!(!format!("{call:?}").contains("private query"));
    assert!(arguments.outputs["query"].ptr_eq(&call.arguments.outputs["query"]));
}

#[test]
fn empty_calls_and_repeated_target_calls_have_independent_responses() {
    let binding = CallBinding::new(
        "clock",
        NodeId::new(4),
        vec![],
        CallInputDefaults::ExplicitOnly,
    )
    .unwrap();
    assert!(binding.ports().is_empty());
    let first = request(4, 20, CallArguments::default());
    let second = request(4, 21, CallArguments::default());
    let first_id = first.id;
    let second_id = second.id;
    let value = ValueRef::from("result").into_secret();
    let second_response = second.respond(CallOutcome::Succeeded(NodeValues::from([(
        "value".into(),
        value.clone(),
    )])));
    let CallOutcome::Succeeded(outputs) = second_response.into_outcome(second_id).unwrap() else {
        panic!("expected successful call");
    };
    assert!(outputs["value"].ptr_eq(&value));
    assert!(!outputs.contains_key("optional"));
    assert!(matches!(
        first.respond(CallOutcome::Succeeded(NodeValues::new())).into_outcome(first_id),
        Ok(CallOutcome::Succeeded(outputs)) if outputs.is_empty()
    ));
}

#[test]
fn responses_reject_both_wrong_bindings_and_wrong_invocations() {
    for (binding, invocation) in [(1, 7), (0, 8)] {
        let call = request(0, 7, CallArguments::default());
        let actual = call.id;
        let expected = CallId {
            binding: CallBindingId::new(binding),
            invocation,
        };
        let error = call
            .respond(CallOutcome::Succeeded(NodeValues::new()))
            .into_outcome(expected)
            .unwrap_err();
        assert!(error.to_string().contains(&format!("{actual:?}")));
        assert!(error.to_string().contains(&format!("{expected:?}")));
        assert!(matches!(error, CallContractError::ResponseMismatch {
            expected: rejected_expected, actual: rejected_actual,
        } if rejected_expected == expected && rejected_actual == actual));
    }
    assert_eq!(CallBindingId::new(9).index(), 9);
}

#[test]
fn skipped_and_failed_calls_preserve_structured_causes() {
    let causes = vec![SkipCause::Output {
        source_node: "config".into(),
        source_output: "token".into(),
    }];
    let skipped = request(0, 0, CallArguments::default());
    let skipped_id = skipped.id;
    assert!(matches!(
        skipped.respond(CallOutcome::Skipped { causes: causes.clone() })
            .into_outcome(skipped_id).unwrap(),
        CallOutcome::Skipped { causes: actual } if actual == causes
    ));

    let source = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let source = WorkflowRunError::NodeExecution {
        definition_id: "tool".into(),
        source: NodePluginFailedSnafu.into_error(Box::new(source)),
    };
    let failed = request(0, 1, CallArguments::default());
    let failed_id = failed.id;
    let CallOutcome::Failed { source } = failed
        .respond(CallOutcome::Failed { source })
        .into_outcome(failed_id)
        .unwrap()
    else {
        panic!("expected failed call");
    };
    let io = source
        .source()
        .unwrap()
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
    assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn contract_errors_propagate_through_node_boundaries() {
    fn prepare() -> Result<CallBinding, NodeBuildError> {
        Ok(CallBinding::new(
            "",
            NodeId::new(0),
            vec![],
            CallInputDefaults::ExplicitOnly,
        )?)
    }
    fn receive() -> Result<CallOutcome, NodeExecutionError> {
        let call = request(0, 1, CallArguments::default());
        let mut expected = call.id;
        expected.invocation = 2;
        Ok(call
            .respond(CallOutcome::Succeeded(NodeValues::new()))
            .into_outcome(expected)?)
    }
    assert!(matches!(
        prepare(),
        Err(NodeBuildError::CallContract {
            source: CallContractError::EmptyName { field: "binding" },
        })
    ));
    assert!(matches!(
        receive(),
        Err(NodeExecutionError::CallContract {
            source: CallContractError::ResponseMismatch { .. },
        })
    ));
}

#[derive(mf_runtime::NodeValue)]
struct Empty {}

#[derive(mf_runtime::NodeValue)]
struct Delta {
    delta: String,
}

struct Combined;
impl mf_runtime::TypedTaskStreamNode for Combined {
    type Input = Empty;
    type Output = Empty;
    type Emission = Delta;

    fn execute(
        &self,
        _: Empty,
        _: &mut mf_runtime::ExecutionContext,
        _: &mut mf_runtime::TypedEmitter<'_, '_, Delta>,
    ) -> Result<mf_runtime::TypedNodeResult<Empty>, NodeExecutionError> {
        panic!("preparation must not execute the node")
    }
}

fn call_metadata(name: &str) -> mf_runtime::NodeMetadata {
    mf_runtime::NodeMetadata {
        call_outputs: vec![mf_runtime::PortSpec::owned(
            name,
            mf_runtime::ValueType::String,
            true,
        )],
        ..Default::default()
    }
}

#[test]
fn call_output_declarations_have_a_separate_validated_namespace() {
    for scenario in ["empty", "duplicate", "ordinary", "stream", "depth"] {
        let mut metadata = call_metadata("query");
        match scenario {
            "empty" => metadata.call_outputs[0].name = "".into(),
            "duplicate" => metadata.call_outputs.push(metadata.call_outputs[0].clone()),
            "ordinary" => metadata
                .ports
                .outputs
                .push(metadata.call_outputs[0].clone()),
            "stream" => metadata
                .stream_outputs
                .push(metadata.call_outputs[0].clone()),
            "depth" => {
                for _ in 0..mf_runtime::ValueType::MAX_DEPTH {
                    metadata.call_outputs[0].value_type = mf_runtime::ValueType::List(Box::new(
                        metadata.call_outputs[0].value_type.clone(),
                    ));
                }
            }
            _ => unreachable!(),
        }
        let error = metadata.validate_call_outputs().unwrap_err();
        assert!(error.to_string().contains("call output"));
        if scenario == "depth" {
            assert!(matches!(error, CallContractError::OutputDepth { .. }));
            assert!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<mf_runtime::TypeDepthError>()
                    .is_some()
            );
        } else {
            assert!(matches!(error, CallContractError::InvalidOutput { .. }));
        }
    }
    let metadata = call_metadata("query");
    metadata.validate_call_outputs().unwrap();
    assert!(metadata.output_ports().next().is_none());
    assert!(mf_runtime::PreparedNode::typed_task_stream(Combined, metadata).is_ok());
    assert!(matches!(
        mf_runtime::PreparedNode::typed_task_stream(Combined, call_metadata("delta")),
        Err(NodeBuildError::CallContract {
            source: CallContractError::InvalidOutput { .. }
        })
    ));
    assert!(matches!(
        mf_runtime::PreparedNode::typed_task_stream(Combined, call_metadata("")),
        Err(NodeBuildError::CallContract { .. })
    ));
}

#[test]
fn generated_metadata_checks_include_call_outputs_without_changing_empty_metadata() {
    assert!(
        serde_json::to_value(mf_runtime::NodeMetadata::default())
            .unwrap()
            .get("call_outputs")
            .is_none()
    );
    let mut metadata = call_metadata("query");
    let expected = serde_json::to_value(&metadata).unwrap();
    mf_runtime::verify_generated_metadata(&metadata, &expected).unwrap();
    metadata.call_outputs[0].value_type = mf_runtime::ValueType::Int64;
    assert!(matches!(
        mf_runtime::verify_generated_metadata(&metadata, &expected),
        Err(NodeBuildError::GenerationMetadataMismatch)
    ));
}
