use mf_runtime::{
    ExecutionContext, ExecutionDependency, FlowNode, GeneratedNodeResult, NodeExecutionError,
    NodeMetadata, NodePorts, NodeResult, Outputs, PortSpec, StreamError, ValueType,
    execute_generated_node_in_context,
};
use serde_json::json;
use std::error::Error;

fn node(id: &str) -> FlowNode<()> {
    FlowNode {
        definition_id: id.into(),
        node: (),
        metadata: NodeMetadata::new(NodePorts::default()),
    }
}
fn after(id: &str) -> ExecutionDependency<'_> {
    ExecutionDependency::NodeSuccess { source_node: id }
}
fn empty(typed: bool) -> GeneratedNodeResult {
    if typed {
        GeneratedNodeResult::Typed {
            produced: vec![],
            skipped: Default::default(),
            loop_summary: None,
        }
    } else {
        GeneratedNodeResult::Dynamic(NodeResult::default())
    }
}

#[test]
fn missing_success_is_typed_and_cancellation_does_not_activate_consumers() {
    for typed in [false, true] {
        let mut context = ExecutionContext::default();
        let error = execute_generated_node_in_context(
            &node("target"),
            [after("source")],
            &mut context,
            &[],
            |_, _| panic!("unresolved source must not invoke target"),
        )
        .unwrap_err();
        assert!(
            matches!(error.source().unwrap().downcast_ref::<NodeExecutionError>(),Some(NodeExecutionError::MissingTaskOutcome { node }) if node=="source")
        );
        execute_generated_node_in_context(&node("source"), [], &mut context, &[], |_, _| {
            Ok(empty(typed))
        })
        .unwrap();
        execute_generated_node_in_context(
            &node("target"),
            [after("source")],
            &mut context,
            &[],
            |_, _| Ok(empty(typed)),
        )
        .unwrap();
        context.cancellation().cancel(StreamError::Execution {
            message: "cancelled source".into(),
        });
        assert!(
            execute_generated_node_in_context(
                &node("later"),
                [after("target")],
                &mut context,
                &[],
                |_, _| panic!("cancellation must prevent target")
            )
            .is_err()
        );
    }
}

#[test]
fn missing_outcome_precedes_a_skip_and_frames_do_not_inherit_success() {
    let mut context = ExecutionContext::default();
    let mut router = node("route");
    router
        .metadata
        .ports
        .outputs
        .push(PortSpec::new("branch", ValueType::Boolean, false));
    execute_generated_node_in_context(&router, [], &mut context, &[], |_, _| {
        Ok(GeneratedNodeResult::Dynamic(NodeResult {
            skipped: ["branch".into()].into(),
            ..Default::default()
        }))
    })
    .unwrap();
    let gate = ExecutionDependency::Output {
        exclusive: false,
        input: None,
        source_node: "route",
        source_output: "branch",
    };
    for deps in [[gate, after("missing")], [after("missing"), gate]] {
        let error =
            execute_generated_node_in_context(&node("target"), deps, &mut context, &[], |_, _| {
                panic!("invalid dependency must not invoke target")
            })
            .unwrap_err();
        assert!(error.to_string().contains("missing task outcome"));
    }
    execute_generated_node_in_context(&node("source"), [], &mut context, &[], |_, _| {
        Ok(empty(true))
    })
    .unwrap();
    for mut child in [context.fork_stream(None), context.fork_body(None)] {
        assert!(
            execute_generated_node_in_context(
                &node("target"),
                [after("source")],
                &mut child,
                &[],
                |_, _| panic!("another frame must not inherit success")
            )
            .is_err()
        );
    }
}

#[test]
fn publication_failure_cannot_leave_a_success_outcome() {
    let mut context = ExecutionContext::default();
    let producer = node("source");
    assert!(
        execute_generated_node_in_context(&producer, [], &mut context, &[], |_, _| Ok(
            GeneratedNodeResult::Dynamic(
                Outputs::from([("unexpected".into(), json!(42).into())]).into()
            )
        ))
        .is_err()
    );
    assert!(
        execute_generated_node_in_context(
            &node("target"),
            [after("source")],
            &mut context,
            &[],
            |_, _| panic!("failed publication is not success")
        )
        .is_err()
    );
}
