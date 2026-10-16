use mf_runtime::{
    MAX_WORKFLOW_INPUT_BYTES, WorkflowArguments, WorkflowInputError, WorkflowInputSchema,
    resolve_workflow_inputs,
};
use serde_json::{Value, json};

fn schema(inputs: Value) -> WorkflowInputSchema {
    serde_json::from_value(json!({"inputs": {"params": inputs}, "stdin": {}})).unwrap()
}

#[test]
fn resolves_sources_in_order_without_reading_overridden_environment() {
    let schema = schema(json!({
        "explicit": {"type": "int", "required": true, "binding": {"env": "EXPLICIT", "default": 1}},
        "environment": {"type": "int", "required": true, "binding": {"env": "NUMBER", "default": 2}},
        "fallback": {"type": "int", "required": true, "binding": {"env": "MISSING", "default": 3}},
        "empty": {"type": "string", "required": true, "binding": {"env": "EMPTY", "default": "unused"}},
        "optional": {"type": "string", "required": false}
    }));
    let arguments = WorkflowArguments::try_from(json!({"params.explicit": 42})).unwrap();
    let mut reads = Vec::new();
    let resolved = resolve_workflow_inputs(&schema, &arguments, |name| {
        reads.push(name.to_owned());
        match name {
            "NUMBER" => Some("7".into()),
            "EMPTY" => Some("".into()),
            "MISSING" => None,
            _ => panic!("unexpected environment lookup: {name}"),
        }
    })
    .unwrap();
    assert_eq!(reads, ["EMPTY", "NUMBER", "MISSING"]);
    assert_eq!(
        serde_json::to_value(&resolved).unwrap(),
        json!({"params.explicit": 42, "params.environment": 7, "params.fallback": 3, "params.empty": ""})
    );
    assert!(resolved.0["params.explicit"].ptr_eq(&arguments.0["params.explicit"]));
    assert_eq!(arguments.0.len(), 1);
}

#[test]
fn supplied_null_and_default_null_are_distinct_from_missing() {
    let schema = schema(json!({
        "explicit": {"type": {"nullable": "string"}, "required": true, "binding": {"env": "UNUSED", "default": "unused"}},
        "fallback": {"type": {"nullable": "string"}, "required": true, "binding": {"default": null}},
        "text": {"type": {"nullable": "string"}, "required": true, "binding": {"env": "TEXT"}}
    }));
    let arguments = WorkflowArguments::try_from(json!({"params.explicit": null})).unwrap();
    for (text, expected) in [("null", Value::Null), ("plain text", json!("plain text"))] {
        let resolved = resolve_workflow_inputs(&schema, &arguments, |name| {
            assert_eq!(name, "TEXT");
            Some(text.into())
        })
        .unwrap();
        assert!(resolved.0["params.explicit"].is_null());
        assert!(resolved.0["params.fallback"].is_null());
        assert_eq!(resolved.0["params.text"], expected);
    }
}

#[test]
fn rejects_invalid_arguments_and_bindings_before_reading_environment() {
    for (input, arguments) in [
        (json!({"env": "VALUE"}), json!({"params.unknown": 1})),
        (json!({"env": "VALUE"}), json!({"params.value": "wrong"})),
        (json!({"env": "VALUE", "default": "wrong"}), json!({})),
        (json!({"env": "INVALID=NAME"}), json!({})),
    ] {
        let schema = schema(json!({
            "value": {"type": "int", "required": true, "binding": input}
        }));
        let arguments = WorkflowArguments::try_from(arguments).unwrap();
        assert!(
            resolve_workflow_inputs(&schema, &arguments, |_| panic!(
                "validation must precede lookup"
            ))
            .is_err()
        );
    }
}

#[test]
fn invalid_environment_values_do_not_fall_back_to_defaults() {
    let schema = schema(json!({
        "value": {"type": "uint", "required": true, "binding": {"env": "VALUE", "default": 1}}
    }));
    for text in ["", "invalid", "-1", "true", "{\"x\":1,\"x\":2}"] {
        let error = resolve_workflow_inputs(&schema, &WorkflowArguments::default(), |_| {
            Some(text.into())
        })
        .unwrap_err();
        assert!(matches!(
            error,
            WorkflowInputError::TextValue { path, .. }
                | WorkflowInputError::TypeMismatch { path, .. }
                if path == "/params.value"
        ));
    }
}

#[test]
fn missing_environment_only_fails_for_required_inputs_without_defaults() {
    let schema = schema(json!({
        "optional": {"type": "string", "required": false, "binding": {"env": "OPTIONAL"}},
        "required": {"type": "string", "required": true, "binding": {"env": "REQUIRED"}}
    }));
    let error =
        resolve_workflow_inputs(&schema, &WorkflowArguments::default(), |_| None).unwrap_err();
    assert!(
        matches!(error, WorkflowInputError::Invalid { path, .. } if path == "/params.required")
    );
    let resolved = resolve_workflow_inputs(&schema, &WorkflowArguments::default(), |name| {
        (name == "REQUIRED").then(|| "present".into())
    })
    .unwrap();
    assert!(!resolved.0.contains_key("params.optional"));
    assert_eq!(resolved.0["params.required"], json!("present"));
}

#[test]
fn marks_secrets_from_every_source_without_mutating_arguments() {
    let schema = schema(json!({
        "explicit": {"type": "object", "required": true, "binding": {"secret": true}},
        "environment": {"type": "object", "required": true, "binding": {"env": "SECRET", "secret": true}},
        "fallback": {"type": "object", "required": true, "binding": {"default": {"token": "default"}, "secret": true}},
        "marked": {"type": "string", "required": true}
    }));
    let mut arguments = WorkflowArguments::try_from(json!({
        "params.explicit": {"token": "explicit"}, "params.marked": "marked"
    }))
    .unwrap();
    let marked = arguments.0.get_mut("params.marked").unwrap();
    *marked = marked.clone().into_secret();
    let resolved = resolve_workflow_inputs(&schema, &arguments, |_| {
        Some(r#"{"token":"environment"}"#.into())
    })
    .unwrap();
    for name in ["explicit", "environment", "fallback"] {
        let value = &resolved.0[&format!("params.{name}")];
        assert!(value.is_secret());
        assert!(value["token"].is_secret());
        assert_eq!(value.redacted(), json!("[REDACTED]"));
    }
    assert!(!arguments.0["params.explicit"].contains_secret());
    assert!(resolved.0["params.marked"].is_secret());
    assert!(resolved.0["params.marked"].ptr_eq(&arguments.0["params.marked"]));
}

#[test]
fn enforces_the_cumulative_environment_byte_limit() {
    let schema = schema(json!({
        "first": {"type": "string", "required": true, "binding": {"env": "FIRST"}},
        "second": {"type": "string", "required": true, "binding": {"env": "SECOND"}}
    }));
    let arguments = WorkflowArguments::default();
    let half = "x".repeat(MAX_WORKFLOW_INPUT_BYTES / 2);
    assert!(resolve_workflow_inputs(&schema, &arguments, |_| Some(half.clone().into())).is_ok());
    let error = resolve_workflow_inputs(&schema, &arguments, |_| Some(format!("{half}x").into()))
        .unwrap_err();
    assert!(matches!(
        error,
        WorkflowInputError::TooLarge { limit } if limit == MAX_WORKFLOW_INPUT_BYTES
    ));
}

#[cfg(unix)]
#[test]
fn non_unicode_environment_reports_the_input_path_without_leaking_the_value() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let schema = schema(json!({
        "a/b~c": {"type": "string", "required": true, "binding": {"env": "SECRET", "default": "unused", "secret": true}}
    }));
    let error = resolve_workflow_inputs(&schema, &WorkflowArguments::default(), |_| {
        Some(OsString::from_vec(b"sentinel-secret\xff".to_vec()))
    })
    .unwrap_err();
    assert!(!format!("{error:?}").contains("sentinel-secret"));
    assert!(matches!(error, WorkflowInputError::Invalid { path, .. } if path == "/params.a~1b~0c"));
}
