use mf_runtime::{
    ExecutionContext, ExecutionScope, Inputs, NodeExecutionError, Outputs, ValueType,
};
use std::collections::BTreeMap;

#[cfg(target_pointer_width = "64")]
#[test]
fn scope_indices_reject_overflow_before_entering_execution() {
    assert!(
        ExecutionScope::new(
            "loop",
            "%loop",
            i64::MAX as usize,
            Outputs::new(),
            BTreeMap::new()
        )
        .is_ok()
    );
    let error = ExecutionScope::new("loop", "%loop", usize::MAX, Outputs::new(), BTreeMap::new())
        .unwrap_err();
    assert!(
        matches!(error, NodeExecutionError::ExecutionFailed { ref message } if message.contains("signed 64-bit range"))
    );
}

#[test]
fn direct_loop_assignments_report_missing_values_without_mutating_context() {
    let node = mf_runtime::prepared_loop_assign("assign", "count", ValueType::Int64);
    let mut context = ExecutionContext::default();
    let error = node
        .node
        .as_ref()
        .unwrap()
        .as_task_node()
        .unwrap()
        .execute(Inputs::new(), &mut context)
        .unwrap_err();
    assert!(
        matches!(error, NodeExecutionError::ExecutionFailed { ref message } if message == "missing assignment value")
    );
    let error = node
        .node
        .as_ref()
        .unwrap()
        .as_task_node()
        .unwrap()
        .execute(Inputs::from([("value".into(), 1i64.into())]), &mut context)
        .unwrap_err();
    assert!(
        matches!(error, NodeExecutionError::ExecutionFailed { ref message } if message.contains("outside a Loop frame"))
    );
}
