use mf_runtime::{
    ContextValue, ExecutionOutputs, FlowNode, GeneratedNodeResult, NodePorts, NodeResult, Outputs,
    PortSpec, ValueRef, ValueType, WorkflowRunError,
};
use std::collections::BTreeSet;

fn node(id: &str) -> FlowNode<()> {
    FlowNode {
        definition_id: id.into(),
        node: (),
        metadata: NodePorts {
            inputs: vec![],
            outputs: vec![
                PortSpec::new("value", ValueType::Int64, true),
                PortSpec::new("optional", ValueType::Int64, false),
            ],
        }
        .into(),
        output_keys: Default::default(),
    }
}

fn result(value: impl Into<ValueRef>) -> NodeResult {
    Outputs::from([("value".into(), value.into())]).into()
}

fn typed(produced: Vec<&'static str>, skipped: &[&str]) -> GeneratedNodeResult {
    GeneratedNodeResult::Typed {
        produced,
        skipped: skipped.iter().map(|name| (*name).to_owned()).collect(),
        loop_summary: None,
    }
}

#[test]
fn rejected_results_preserve_existing_dynamic_and_typed_outputs() {
    let node = node("source");
    let mut state = ExecutionOutputs::default();
    let original = ValueRef::from(7).into_secret();
    state
        .publish(&node, Some(result(original.clone())))
        .unwrap();
    state
        .publish_generated(&node, Some(typed(vec!["optional"], &[])))
        .unwrap();

    let invalid = NodeResult {
        outputs: Outputs::from([
            ("optional".into(), 9.into()),
            ("value".into(), "wrong".into()),
        ]),
        ..Default::default()
    };
    let error = state.publish(&node, Some(invalid)).unwrap_err();
    assert!(matches!(error, WorkflowRunError::OutputType { .. }));

    for invalid in [
        typed(vec!["value", "undeclared"], &[]),
        typed(vec!["optional"], &["optional"]),
        typed(vec![], &["value"]),
    ] {
        assert!(state.publish_generated(&node, Some(invalid)).is_err());
    }
    for invalid in [
        NodeResult {
            outputs: Outputs::from([("undeclared".into(), 1.into())]),
            ..Default::default()
        },
        NodeResult {
            outputs: Outputs::from([("optional".into(), 1.into())]),
            skipped: BTreeSet::from(["optional".into()]),
            ..Default::default()
        },
        NodeResult {
            skipped: BTreeSet::from(["value".into()]),
            ..Default::default()
        },
    ] {
        assert!(state.publish(&node, Some(invalid)).is_err());
    }
    let ContextValue::Value(value) = state.output("source.value").unwrap() else {
        panic!("failed publication replaced the previous value");
    };
    assert!(value.ptr_eq(&original));
    assert!(value.is_secret());
    assert!(state.is_transferred("source.optional"));
}

#[test]
fn publication_transitions_between_dynamic_typed_and_skipped_outputs() {
    let node = node("source");
    let mut state = ExecutionOutputs::default();
    state.publish(&node, Some(result(1))).unwrap();
    assert!(state.output_available("source", "value").unwrap());
    state
        .publish_generated(&node, Some(typed(vec!["value"], &["optional"])))
        .unwrap();
    assert!(state.output_available("source", "value").unwrap());
    assert!(state.is_transferred("source.value"));
    assert!(state.output("source.value").is_err());
    assert_eq!(
        state.output("source.optional").unwrap(),
        ContextValue::Skipped
    );

    state
        .publish_generated(&node, Some(GeneratedNodeResult::Dynamic(result(2))))
        .unwrap();
    assert!(!state.is_transferred("source.value"));
    assert_eq!(
        state.output("source.value").unwrap(),
        ContextValue::Value(&ValueRef::from(2))
    );
    state
        .publish_generated(&node, Some(typed(vec!["value", "optional"], &[])))
        .unwrap();
    state.publish_generated(&node, None).unwrap();
    for port in ["value", "optional"] {
        assert!(!state.output_available("source", port).unwrap());
        assert!(!state.is_transferred(&format!("source.{port}")));
    }
}

#[test]
fn visible_forks_share_values_preserve_skips_and_isolate_updates() {
    let original = ValueRef::from(42).into_secret();
    let mut parent = ExecutionOutputs::from_values(Outputs::from([
        ("source.value".into(), original.clone()),
        ("hidden.value".into(), 9.into()),
    ]));
    parent.publish(&node("skipped"), None).unwrap();
    parent
        .publish_generated(&node("typed"), Some(typed(vec!["value"], &[])))
        .unwrap();
    let mut child = parent.fork_snapshot(Some(mf_runtime::OutputVisibility::from_sorted(vec![
        "missing.value".into(),
        "skipped.value".into(),
        "source.value".into(),
    ])));
    let ContextValue::Value(value) = child.output("source.value").unwrap() else {
        panic!("visible value must remain available");
    };
    assert!(value.ptr_eq(&original));
    assert!(value.is_secret());
    assert_eq!(
        child.output("skipped.value").unwrap(),
        ContextValue::Skipped
    );
    assert!(child.output("hidden.value").is_err());
    assert!(child.output("missing.value").is_err());
    assert!(child.is_transferred("typed.value"));
    child.set_value("source.value".into(), 99.into());
    assert_eq!(
        parent.output("source.value").unwrap(),
        ContextValue::Value(&original)
    );
}

#[test]
fn merging_only_commits_selected_nodes_and_replaces_output_representations() {
    let selected = node("selected");
    let untouched = node("untouched");
    let mut parent = ExecutionOutputs::default();
    parent.publish(&selected, Some(result(1))).unwrap();
    parent.publish(&untouched, Some(result(2))).unwrap();
    let mut child = parent.clone();
    child
        .publish_generated(&selected, Some(typed(vec!["value"], &["optional"])))
        .unwrap();
    child.publish(&untouched, Some(result(3))).unwrap();
    parent.merge_nodes(&child, [&selected]);
    assert!(parent.is_transferred("selected.value"));
    assert!(parent.output("selected.value").is_err());
    assert_eq!(
        parent.output("selected.optional").unwrap(),
        ContextValue::Skipped
    );
    assert_eq!(
        parent.output("untouched.value").unwrap(),
        ContextValue::Value(&ValueRef::from(2))
    );

    let secret = ValueRef::from(4).into_secret();
    child
        .publish(&selected, Some(result(secret.clone())))
        .unwrap();
    parent.merge_nodes(&child, [&selected]);
    assert!(!parent.is_transferred("selected.value"));
    let ContextValue::Value(value) = parent.output("selected.value").unwrap() else {
        panic!("merged dynamic value must be available");
    };
    assert!(value.ptr_eq(&secret));
    assert!(value.is_secret());
    parent.merge_nodes(&ExecutionOutputs::default(), [&selected]);
    assert_eq!(
        parent.output("selected.value").unwrap(),
        ContextValue::Value(&secret)
    );
}

#[test]
fn output_selection_distinguishes_missing_skipped_and_null_values() {
    let mut state = ExecutionOutputs::default();
    assert!(matches!(
        state.select_output("answer", "source", "optional", true),
        Err(WorkflowRunError::OutputSelection { .. })
    ));
    state.publish(&node("source"), None).unwrap();
    assert_eq!(
        state
            .select_output("answer", "source", "optional", true)
            .unwrap(),
        None
    );
    assert!(matches!(
        state.select_output("answer", "source", "optional", false),
        Err(WorkflowRunError::Context { .. })
    ));
    state.set_value("source.optional".into(), ValueRef::null());
    assert_eq!(
        state
            .select_output("answer", "source", "optional", true)
            .unwrap(),
        Some(ValueRef::null())
    );
}

#[test]
fn snapshots_freeze_parent_updates_and_keep_branch_writes_private() {
    let mut parent = ExecutionOutputs::from_values(Outputs::from([
        ("source.value".into(), 1.into()),
        ("sibling.value".into(), 2.into()),
    ]));
    let visibility = mf_runtime::OutputVisibility::Borrowed(&["source.value"]);
    assert_eq!(
        visibility,
        mf_runtime::OutputVisibility::from_sorted(vec!["source.value".into()])
    );
    let mut left = parent.fork_snapshot(Some(visibility.clone()));
    let right = parent.fork_snapshot(Some(visibility));
    parent.set_value("source.value".into(), 3.into());
    parent.set_value("later.value".into(), 4.into());
    left.set_value("source.value".into(), 5.into());
    left.publish(&node("left"), Some(result(6))).unwrap();
    assert_eq!(
        right.output("source.value").unwrap(),
        ContextValue::Value(&1.into())
    );
    assert_eq!(
        left.output("source.value").unwrap(),
        ContextValue::Value(&5.into())
    );
    assert_eq!(
        parent.output("source.value").unwrap(),
        ContextValue::Value(&3.into())
    );
    for snapshot in [&left, &right] {
        assert!(snapshot.output("sibling.value").is_err());
        assert!(snapshot.output("later.value").is_err());
    }
    assert!(right.output("left.value").is_err());
    assert!(parent.output("left.value").is_err());
    parent.merge_nodes(&left, [&node("left")]);
    assert_eq!(
        parent.output("left.value").unwrap(),
        ContextValue::Value(&6.into())
    );
    assert_eq!(
        parent.output("source.value").unwrap(),
        ContextValue::Value(&3.into())
    );
}

#[test]
fn nested_snapshots_cannot_restore_hidden_values_and_shadow_inherited_states() {
    let mut parent = ExecutionOutputs::default();
    parent.publish(&node("source"), Some(result(1))).unwrap();
    parent.publish(&node("hidden"), Some(result(2))).unwrap();
    parent
        .publish_generated(&node("typed"), Some(typed(vec!["optional"], &[])))
        .unwrap();
    let first = parent.fork_snapshot(Some(mf_runtime::OutputVisibility::Borrowed(&[
        "source.value",
    ])));
    let mut nested = first.fork_snapshot(Some(mf_runtime::OutputVisibility::Borrowed(&[
        "hidden.value",
        "source.value",
    ])));
    assert!(nested.output("hidden.value").is_err());
    nested.publish(&node("source"), None).unwrap();
    assert_eq!(
        nested.output("source.value").unwrap(),
        ContextValue::Skipped
    );
    assert_eq!(
        first.output("source.value").unwrap(),
        ContextValue::Value(&1.into())
    );
    nested.publish(&node("typed"), Some(result(3))).unwrap();
    assert!(!nested.is_transferred("typed.optional"));
    assert!(!nested.contains_output("typed.optional"));
    assert!(parent.is_transferred("typed.optional"));
}
