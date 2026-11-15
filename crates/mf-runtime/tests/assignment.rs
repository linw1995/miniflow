use mf_runtime::{
    ExecutionContext, ExecutionScope, GeneratedNodeResult, Inputs, NodeBuildError,
    NodeStateContract, Outputs, StateTarget, ValueRef, ValueType, WorkflowRunError,
    WorkflowVariableDefinition, execute_generated_node_in_context, prepared_assign, root_scope,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error, sync::Arc};

struct State {
    initial: Outputs,
    value_type: ValueType,
}

impl NodeStateContract for State {
    fn initial(&self) -> Outputs {
        self.initial.clone()
    }
    fn writable_type(&self, field: &str) -> Option<ValueType> {
        (field == "count").then(|| self.value_type.clone())
    }
}

fn state(initial: Outputs, value_type: ValueType) -> Arc<dyn NodeStateContract> {
    Arc::new(State {
        initial,
        value_type,
    })
}

fn publish(
    target: &StateTarget,
    context: &mut ExecutionContext,
    value: ValueRef,
    invalid_output: bool,
) -> Result<(), WorkflowRunError> {
    let contract = matches!(target, StateTarget::Node { .. }).then(|| {
        state(
            Outputs::from([("count".into(), 0.into())]),
            ValueType::Int64,
        )
    });
    let mut node = prepared_assign("write", target.clone(), ValueType::Int64, contract)
        .unwrap()
        .into_task()
        .unwrap();
    assert_eq!(
        node.node.supports_success_dependencies(),
        matches!(target, StateTarget::Node { .. })
    );
    node.metadata.ports.inputs.clear();
    if invalid_output {
        node.metadata.ports.outputs[0].value_type = ValueType::String;
    }
    execute_generated_node_in_context(&node, [], context, &[], |_, context| {
        node.node
            .execute(Inputs::from([("value".into(), value)]), context)
            .map(GeneratedNodeResult::Dynamic)
    })
}

#[test]
fn all_targets_share_publication_rollback_and_preserve_sensitive_values() {
    for target in [
        StateTarget::Root {
            field: "count".into(),
        },
        StateTarget::Frame {
            field: "count".into(),
        },
        StateTarget::Node {
            node: "owner".into(),
            field: "count".into(),
        },
    ] {
        let mut context = ExecutionContext::default();
        let source = match &target {
            StateTarget::Root { .. } => "%root.count",
            StateTarget::Frame { .. } => "%actual-frame.count",
            StateTarget::Node { .. } => "%owner.count",
        };
        let check = |context: &mut ExecutionContext| -> Result<(), WorkflowRunError> {
            let secret = ValueRef::from(5).into_secret();
            publish(&target, context, secret.clone(), false)?;
            let mf_runtime::ContextValue::Value(value) = context.output(source).unwrap() else {
                panic!("expected a value")
            };
            assert!(value.is_secret());
            assert!(value.ptr_eq(&secret));
            assert!(publish(&target, context, 6.into(), true).is_err());
            let mf_runtime::ContextValue::Value(value) = context.output(source).unwrap() else {
                panic!("expected a value")
            };
            assert!(value.ptr_eq(&secret));
            let error = publish(&target, context, "wrong".into(), false).unwrap_err();
            let mut source: &(dyn Error + 'static) = &error;
            loop {
                if let Some(mismatch) = source.downcast_ref::<Box<mf_runtime::TypeMismatch>>() {
                    assert_eq!(mismatch.expected, ValueType::Int64);
                    break;
                }
                source = source
                    .source()
                    .expect("type mismatch must retain its source");
            }
            Ok(())
        };
        match &target {
            StateTarget::Root { .. } => {
                let root = root_scope(
                    &[WorkflowVariableDefinition {
                        name: "count".into(),
                        value_type: json!("int"),
                        initial: json!(0),
                    }],
                    "%root".into(),
                )
                .unwrap();
                context.run_root_scope(&root, check).unwrap();
            }
            StateTarget::Frame { .. } => {
                let frame = ExecutionScope::new(
                    "repeat",
                    "%actual-frame",
                    0,
                    Outputs::from([("count".into(), 0.into())]),
                    BTreeMap::from([("count".into(), ValueType::Int64)]),
                )
                .unwrap();
                context.run_scope(frame, check).unwrap();
            }
            StateTarget::Node { .. } => check(&mut context).unwrap(),
        }
    }
}

#[test]
fn assignment_construction_checks_provider_ownership_and_initial_types() {
    let node = StateTarget::Node {
        node: "owner".into(),
        field: "count".into(),
    };
    assert!(matches!(
        prepared_assign("write", node.clone(), ValueType::Int64, None),
        Err(NodeBuildError::StateContract)
    ));
    let root = StateTarget::Root {
        field: "count".into(),
    };
    assert!(matches!(
        prepared_assign(
            "write",
            root,
            ValueType::Int64,
            Some(state(Outputs::new(), ValueType::Int64))
        ),
        Err(NodeBuildError::StateContract)
    ));
    assert!(matches!(
        prepared_assign(
            "write",
            node.clone(),
            ValueType::Int64,
            Some(state(Outputs::new(), ValueType::Int64))
        ),
        Err(NodeBuildError::StateField { .. })
    ));
    assert!(matches!(
        prepared_assign(
            "write",
            node.clone(),
            ValueType::Int64,
            Some(state(
                Outputs::from([("count".into(), true.into())]),
                ValueType::Int64
            ))
        ),
        Err(NodeBuildError::StateInitial { .. })
    ));
    assert!(matches!(
        prepared_assign(
            "write",
            node,
            ValueType::Boolean,
            Some(state(
                Outputs::from([("count".into(), 0.into())]),
                ValueType::Int64
            ))
        ),
        Err(NodeBuildError::StateContract)
    ));
}
