use mf_runtime::{
    ExecutionContext, Inputs, NodeExecutionError, Outputs, ROOT_SOURCE_ID, RootScopeRelations,
    ValueRef, ValueType, WorkflowRunError, WorkflowVariableDefinition,
    execute_generated_node_in_context, execute_node_in_context, prepared_root_assign,
    prepared_root_source, root_scope,
};
use serde_json::json;
use snafu::ResultExt;
use std::{
    error::Error,
    panic::{AssertUnwindSafe, catch_unwind},
};

fn variable(
    name: &str,
    value_type: serde_json::Value,
    initial: serde_json::Value,
) -> WorkflowVariableDefinition {
    WorkflowVariableDefinition {
        name: name.into(),
        value_type,
        initial,
    }
}

fn scope() -> RootScopeRelations {
    root_scope(
        &[variable("count", json!("int"), json!(7))],
        ROOT_SOURCE_ID.into(),
    )
    .unwrap()
}

fn current(context: &mut ExecutionContext, scope: &RootScopeRelations) -> Outputs {
    let source = prepared_root_source(scope.input.as_str(), &scope.values)
        .into_task()
        .unwrap();
    source.node.execute(Inputs::new(), context).unwrap().outputs
}

fn write(
    context: &mut ExecutionContext,
    value: ValueRef,
    invalid_output: bool,
) -> Result<(), WorkflowRunError> {
    let mut assign = prepared_root_assign("write", "count", ValueType::Any)
        .into_task()
        .unwrap();
    // Supply inputs directly to the invocation while retaining ordinary publication validation.
    assign.metadata.ports.inputs.clear();
    if invalid_output {
        assign.metadata.ports.outputs[0].value_type = ValueType::String;
    }
    execute_generated_node_in_context(&assign, [], context, &[], |_, context| {
        assign
            .node
            .execute(Inputs::from([("value".into(), value)]), context)
            .map(mf_runtime::GeneratedNodeResult::Dynamic)
    })
}

#[test]
fn root_state_writes_commit_only_with_valid_publication_and_preserve_typed_sources() {
    let scope = scope();
    let mut context = ExecutionContext::default();
    context
        .run_root_scope(&scope, |context| {
            assert!(context.scope_path().is_empty());
            assert!(context.invocation_path().is_empty());
            write(context, 8i64.into(), false)?;
            assert_eq!(current(context, &scope)["count"], json!(8));
            assert_eq!(
                context
                    .select_output("value", ROOT_SOURCE_ID, "count", false)?
                    .unwrap(),
                json!(8)
            );
            assert!(write(context, 9i64.into(), true).is_err());
            assert_eq!(current(context, &scope)["count"], json!(8));
            let error = write(context, json!("wrong").into(), false).unwrap_err();
            let mut source: &(dyn Error + 'static) = &error;
            loop {
                if let Some(mismatch) = source.downcast_ref::<Box<mf_runtime::TypeMismatch>>() {
                    assert_eq!(mismatch.expected, ValueType::Int64);
                    assert_eq!(mismatch.actual, "string");
                    break;
                }
                source = source
                    .source()
                    .expect("typed mismatch must remain in the source chain");
            }
            assert_eq!(current(context, &scope)["count"], json!(8));
            Ok(())
        })
        .unwrap();
    let node = prepared_root_source(ROOT_SOURCE_ID, &scope.values)
        .into_task()
        .unwrap();
    assert!(node.node.execute(Inputs::new(), &mut context).is_err());
}

#[test]
fn root_assignments_reject_missing_scopes_and_unknown_targets_without_publishing() {
    let scope = scope();
    let mut context = ExecutionContext::default();
    let assign = prepared_root_assign("write", "missing", ValueType::Int64)
        .into_task()
        .unwrap();
    let inputs = || Inputs::from([("value".into(), 8i64.into())]);
    let error = assign.node.execute(inputs(), &mut context).unwrap_err();
    assert!(
        matches!(error, NodeExecutionError::ExecutionFailed { message }
        if message.contains("outside a Flow state scope"))
    );
    context
        .run_root_scope(&scope, |context| {
            let error = assign.node.execute(inputs(), context).unwrap_err();
            assert!(
                matches!(error, NodeExecutionError::ExecutionFailed { message }
                if message == "unknown workflow variable `missing`")
            );
            assert!(context.output("write.done").is_err());
            assert_eq!(current(context, &scope)["count"], json!(7));
            Ok(())
        })
        .unwrap();
}

#[test]
fn root_state_preserves_native_payloads_and_sensitivity_only_writes_across_domains() {
    let scope = scope();
    let mut context = ExecutionContext::default();
    context
        .run_root_scope(&scope, |context| {
            let initial = current(context, &scope).remove("count").unwrap();
            let secret = initial.into_secret();
            let mut fork = context.fork_domain_with_observation(None);
            write(&mut fork, secret.clone(), false)?;
            context.merge_domain_outputs(
                &fork,
                [&prepared_root_assign("write", "count", ValueType::Int64)],
            );
            let updated = current(context, &scope).remove("count").unwrap();
            assert!(updated.is_secret());
            assert!(updated.ptr_eq(&secret));
            let value = context
                .select_output("final", ROOT_SOURCE_ID, "count", false)?
                .unwrap();
            assert!(value.is_secret());
            assert!(value.ptr_eq(&secret));
            Ok(())
        })
        .unwrap();
}

#[test]
fn root_state_is_fresh_and_unwound_on_success_failure_and_panic() {
    let scope = scope();
    let mut context = ExecutionContext::default();
    for outcome in 0..3 {
        let result = catch_unwind(AssertUnwindSafe(|| {
            context.run_root_scope(&scope, |context| {
                assert_eq!(current(context, &scope)["count"], json!(7));
                write(context, 42i64.into(), false)?;
                match outcome {
                    0 => Ok(()),
                    1 => Err(mf_runtime::NodeExecutionFailedSnafu {
                        message: "expected failure",
                    }
                    .build())
                    .context(mf_runtime::NodeExecutionSnafu {
                        definition_id: "failure",
                    }),
                    _ => panic!("expected unwind"),
                }
            })
        }));
        assert_eq!(result.is_err(), outcome == 2);
        let node = prepared_root_source(ROOT_SOURCE_ID, &scope.values)
            .into_task()
            .unwrap();
        let error = node.node.execute(Inputs::new(), &mut context).unwrap_err();
        assert!(matches!(error, NodeExecutionError::ExecutionFailed { .. }));
        // Resetting a frame must also clear any staged write from a previous failure.
        context
            .run_root_scope(&scope, |context| {
                execute_node_in_context(&node, &[], context)?;
                assert_eq!(current(context, &scope)["count"], json!(7));
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn root_assignment_nested_mismatch_keeps_the_exact_path_and_previous_value() {
    let mut scope = scope();
    scope.values.insert(
        "count".into(),
        ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64)))),
    );
    scope.initial.insert("count".into(), json!([]));
    let mut context = ExecutionContext::default();
    context
        .run_root_scope(&scope, |context| {
            let error = write(context, json!([{"count":"wrong"}]).into(), false).unwrap_err();
            let WorkflowRunError::NodeExecution { source, .. } = error else {
                panic!("expected node execution error")
            };
            let NodeExecutionError::WorkflowVariableType { source, variable } = source else {
                panic!("expected workflow variable mismatch")
            };
            assert_eq!(variable, "count");
            assert_eq!(source.path, "/0/count");
            assert_eq!(source.expected, ValueType::Int64);
            assert_eq!(source.actual, "string");
            assert_eq!(current(context, &scope)["count"], json!([]));
            assert!(context.output("write.done").is_err());
            Ok(())
        })
        .unwrap();
}

#[test]
fn root_initials_are_required_even_when_null_and_index_is_an_ordinary_variable() {
    // An omitted field is not an implicit null; an explicit null is retained.
    assert!(
        serde_json::from_value::<WorkflowVariableDefinition>(json!({"name":"value", "type":"any"}))
            .is_err()
    );
    let scope = root_scope(
        &[
            variable("value", json!("null"), json!(null)),
            variable("index", json!("string"), json!("not a loop")),
        ],
        ROOT_SOURCE_ID.into(),
    )
    .unwrap();
    let source = prepared_root_source(ROOT_SOURCE_ID, &scope.values);
    assert_eq!(source.metadata.ports.outputs.len(), 2);
    assert_eq!(
        source.metadata.ports.outputs[0].value_type,
        ValueType::String
    );
    let mut context = ExecutionContext::default();
    context
        .run_root_scope(&scope, |context| {
            assert_eq!(
                serde_json::to_value(current(context, &scope)).unwrap(),
                json!({"index":"not a loop", "value":null})
            );
            Ok(())
        })
        .unwrap();
}
