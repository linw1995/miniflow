use mf_runtime::{
    CallArguments, CallBinding, CallBindingId, CallContractError, CallId, CallInputDefaults,
    CallOutcome, CallRequest, NodeBuildError, NodeExecutionError, NodeId, NodePluginFailedSnafu,
    NodeValues, SkipCause, ValueRef, WorkflowRunError,
};
use snafu::IntoError;
use std::error::Error;

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
fn bindings_declare_workflow_identity_and_default_policy_without_port_mappings() {
    let binding = CallBinding::new("search", NodeId::new(3), CallInputDefaults::ExplicitOnly)
        .unwrap()
        .with_description("Search a workflow");
    assert_eq!(binding.name(), "search");
    assert_eq!(binding.target(), NodeId::new(3));
    assert_eq!(binding.defaults(), CallInputDefaults::ExplicitOnly);
    assert_eq!(binding.description(), Some("Search a workflow"));
    assert!(matches!(
        CallBinding::new(" ", NodeId::new(0), CallInputDefaults::ExplicitOnly),
        Err(CallContractError::EmptyName { field: "binding" })
    ));
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
fn outcomes_preserve_shared_values_omission_and_typed_failure_sources() {
    let value = ValueRef::from("result").into_secret();
    let success = CallOutcome::Succeeded(NodeValues::from([("value".into(), value.clone())]));
    let CallOutcome::Succeeded(outputs) = success else {
        panic!("expected success")
    };
    assert!(outputs["value"].ptr_eq(&value));
    assert!(!outputs.contains_key("optional"));
    let causes = vec![SkipCause::Output {
        source_node: "config".into(),
        source_output: "token".into(),
    }];
    assert!(
        matches!(CallOutcome::Skipped { causes: causes.clone() }, CallOutcome::Skipped { causes: actual } if actual == causes)
    );
    let source = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let outcome = CallOutcome::Failed {
        source: WorkflowRunError::NodeExecution {
            definition_id: "tool".into(),
            source: NodePluginFailedSnafu.into_error(Box::new(source)),
        },
    };
    let CallOutcome::Failed { source } = outcome else {
        panic!("expected failure")
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
            CallInputDefaults::ExplicitOnly,
        )?)
    }
    fn execute() -> Result<CallBinding, NodeExecutionError> {
        Ok(CallBinding::new(
            "",
            NodeId::new(0),
            CallInputDefaults::ExplicitOnly,
        )?)
    }
    assert!(matches!(
        prepare(),
        Err(NodeBuildError::CallContract {
            source: CallContractError::EmptyName { field: "binding" },
        })
    ));
    assert!(matches!(
        execute(),
        Err(NodeExecutionError::CallContract {
            source: CallContractError::EmptyName { field: "binding" },
        })
    ));
}
