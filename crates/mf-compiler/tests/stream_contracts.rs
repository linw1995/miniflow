use mf_runtime::{StreamLimits, WorkflowDefinition};
use serde_json::json;

#[test]
fn streaming_schema_validates_versions_types_and_limits() {
    let original = json!({"version":"2026-10-03", "dependencies":{}, "nodes":[],
        "execution":{"mode":"stream"}});
    let definition = WorkflowDefinition::from_json(&original.to_string()).unwrap();
    let execution = definition.execution.as_ref().unwrap();
    assert_eq!(execution.limits, StreamLimits::default());
    let invalid_limits = StreamLimits {
        workers: 0,
        ..StreamLimits::default()
    };
    assert!(matches!(
        mf_compiler::validate_stream_limits(&invalid_limits),
        Err(mf_compiler::StreamBuildError::InvalidPlan { message }) if message == "stream limits must be positive"
    ));
    assert_eq!(
        definition,
        serde_json::from_value(serde_json::to_value(&definition).unwrap()).unwrap()
    );

    for invalid in [json!(0), json!(-1), json!(1.5), json!("4")] {
        for field in ["max_pending_messages", "workers"] {
            let mut value = original.clone();
            value["execution"]["limits"] = json!({field: invalid});
            assert!(
                WorkflowDefinition::from_json(&value.to_string()).is_err()
                    || mf_compiler::validate_execution(&serde_json::from_value(value).unwrap())
                        .is_err()
            );
        }
    }
    for version in ["2026-09-26", "2026-09-29", "2026-10-02"] {
        let mut value = original.clone();
        value["version"] = json!(version);
        assert!(
            mf_compiler::validate_execution(
                &WorkflowDefinition::from_json(&value.to_string()).unwrap()
            )
            .unwrap_err()
            .to_string()
            .contains("2026-10-03")
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
        json!(null),
        json!({"mode":"unknown", "input_type":"int"}),
        json!({"mode":"stream", "input_type":"invalid"}),
        json!({"mode":"stream", "input_type":"int", "unknown":true}),
        json!({"mode":"stream", "input_type":"int", "limits":{"unknown":1}}),
    ] {
        let mut value = original.clone();
        value["execution"] = execution;
        assert!(WorkflowDefinition::from_json(&value.to_string()).is_err());
    }
}
