use mf_runtime::{
    Inputs, NodeExecutionError, Outputs, StreamLimits, ValueType, WorkflowDefinition,
};
use serde_json::json;

#[test]
fn streaming_schema_validates_versions_types_and_limits() {
    let original = json!({"version":"2026-10-02", "dependencies":{}, "nodes":[],
        "execution":{"mode":"stream", "input_type":{"list":"int"}}});
    let definition: WorkflowDefinition = serde_json::from_value(original.clone()).unwrap();
    let execution = definition.execution.as_ref().unwrap();
    assert_eq!(
        execution.input_type,
        ValueType::List(Box::new(ValueType::Int64))
    );
    assert_eq!(execution.limits, StreamLimits::default());
    assert_eq!(
        definition,
        serde_json::from_value(serde_json::to_value(&definition).unwrap()).unwrap()
    );

    for invalid in [json!(0), json!(-1), json!(1.5), json!("4")] {
        for field in [
            "max_pending_messages",
            "max_buffered_bytes",
            "max_message_bytes",
            "workers",
        ] {
            let mut value = original.clone();
            value["execution"]["limits"] = json!({field: invalid});
            assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
        }
    }
    for version in ["2026-09-26", "2026-09-29"] {
        let mut value = original.clone();
        value["version"] = json!(version);
        assert!(
            serde_json::from_value::<WorkflowDefinition>(value.clone())
                .unwrap_err()
                .to_string()
                .contains("2026-10-02")
        );
        value.as_object_mut().unwrap().remove("execution");
        let legacy: WorkflowDefinition = serde_json::from_value(value).unwrap();
        assert!(
            serde_json::to_value(legacy)
                .unwrap()
                .get("execution")
                .is_none()
        );
    }
    for execution in [
        json!({"mode":"stream"}),
        json!({"mode":"unknown", "input_type":"int"}),
        json!({"mode":"stream", "input_type":"invalid"}),
        json!({"mode":"stream", "input_type":"int", "unknown":true}),
        json!({"mode":"stream", "input_type":"int", "limits":{"unknown":1}}),
        json!({"mode":"stream", "input_type":"int", "limits":{"max_buffered_bytes":10}}),
    ] {
        let mut value = original.clone();
        value["execution"] = execution;
        assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
    }
}

#[test]
fn cancellation_during_a_call_prevents_publication_of_its_returned_values() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct CancelOnReturn(Arc<AtomicBool>);
    impl mf_runtime::TaskNode for CancelOnReturn {
        fn execute(
            &self,
            _: Inputs,
            _ctx: &mut mf_runtime::ExecutionContext,
        ) -> Result<mf_runtime::NodeResult, NodeExecutionError> {
            self.0.store(true, Ordering::Release);
            Ok((Outputs::from([("value".into(), json!(42).into())])).into())
        }
    }
    let flag = Arc::new(AtomicBool::new(false));
    let mut context = mf_runtime::ExecutionContext::default();
    context.set_cancellation(Arc::clone(&flag));
    let node = mf_runtime::FlowNode::new(
        "cancel",
        mf_runtime::PreparedNode::new(
            CancelOnReturn(flag),
            mf_runtime::NodePorts {
                inputs: Vec::new(),
                outputs: vec![mf_runtime::PortSpec::new("value", ValueType::Int64, true)],
            },
        ),
    )
    .into_task()
    .unwrap();
    let error = mf_runtime::execute_node_in_context(&node, &[], &mut context).unwrap_err();
    assert!(error.to_string().contains("cancelled before publication"));
    assert!(context.output("cancel.value").is_err());
}
