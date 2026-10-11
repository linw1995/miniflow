use mf_runtime::{WorkflowArguments, WorkflowInputSchema};
use serde_json::json;

#[test]
fn runner_verbosity_preserves_workflow_values_and_transport_validation() {
    for (flags, expected) in [
        (vec!["-v", "--read.part.path", "-v"], "-v"),
        (vec!["--read.part.path=--verbose", "-v"], "--verbose"),
        (vec!["-v", "--read.part.path", "-vv"], "-vv"),
        (vec!["--read.part.path", "-vvv", "-v"], "-vvv"),
        (
            vec!["--verbose", "--inputs", r#"{"read.part.path":"x"}"#],
            "x",
        ),
    ] {
        let options =
            mf_runtime::RunnerArguments::parse(flags.iter().map(Into::into), &schema()).unwrap();
        assert_eq!(options.verbose, 1);
        assert_eq!(options.inputs.0["read.part.path"], json!(expected));
    }
    for flags in [
        vec!["--verbose=true", "--read.part.path=x"],
        vec!["--verbose", "--inputs", "{}", "--read.part.path=x"],
    ] {
        assert!(
            mf_runtime::RunnerArguments::parse(flags.iter().map(Into::into), &schema()).is_err()
        );
    }
}

#[test]
fn runner_verbosity_accumulates_clusters_and_caps_at_full_records() {
    for (flags, expected) in [
        (vec![], 0),
        (vec!["-vv"], 2),
        (vec!["-v", "--verbose"], 2),
        (vec!["-vvv"], 3),
        (vec!["--verbose", "--verbose", "--verbose"], 3),
        (vec!["-vvvv", "-vv"], 3),
    ] {
        let options = mf_runtime::RunnerArguments::parse(
            flags.into_iter().map(Into::into),
            &WorkflowInputSchema::default(),
        )
        .unwrap();
        assert_eq!(options.verbose, expected);
    }
    for flag in ["-vX", "-vv=2", "-", "--vv"] {
        assert!(
            mf_runtime::RunnerArguments::parse([flag.into()], &WorkflowInputSchema::default())
                .is_err()
        );
    }
}

fn schema() -> WorkflowInputSchema {
    serde_json::from_value(json!({
        "inputs": {
            "read.part": {
                "path": {"type":"string", "required":true},
                "count": {"type":"uint", "required":false},
                "enabled": {"type":"bool", "required":false},
                "items": {"type":{"list":"int"}, "required":false},
                "object": {"type":"object", "required":false},
                "label": {"type":{"nullable":"string"}, "required":false}
            }
        },
        "stdin": {}
    }))
    .unwrap()
}

#[test]
fn named_parameters_preserve_types_and_complete_qualified_names() {
    let schema = schema();
    let arguments = WorkflowArguments::parse(
        [
            "--read.part.path",
            "123 true.txt",
            "--read.part.count=18446744073709551615",
            "--read.part.enabled",
            "false",
            "--read.part.items",
            "[1,-2]",
            "--read.part.object",
            "{\"nested\":[null,true]}",
            "--read.part.label",
            "null",
        ]
        .map(Into::into),
        &schema,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&arguments).unwrap(),
        json!({
            "read.part.path":"123 true.txt",
            "read.part.count":u64::MAX,
            "read.part.enabled":false,
            "read.part.items":[1,-2],
            "read.part.object":{"nested":[null,true]},
            "read.part.label":null
        })
    );
    let text = WorkflowArguments::parse(
        ["--read.part.path=--literal", "--read.part.label=plain text"].map(Into::into),
        &schema,
    )
    .unwrap();
    assert_eq!(text.0["read.part.path"], json!("--literal"));
    assert_eq!(text.0["read.part.label"], json!("plain text"));
}

#[test]
fn rejects_invalid_named_parameters_and_mixed_transports() {
    let schema = schema();
    for arguments in [
        vec![],
        vec!["--read.part.path"],
        vec!["--read.part.path", "--read.part.enabled", "true"],
        vec!["--read.part.unknown", "value"],
        vec!["--read.part.path=x", "--read.part.path=y"],
        vec!["--read.part.path=x", "--read.part.count=-1"],
        vec!["--read.part.path=x", "--read.part.enabled=1"],
        vec!["--read.part.path=x", "--read.part.items=[\"wrong\"]"],
        vec!["--read.part.path=x", "--read.part.object={\"a\":1,\"a\":2}"],
        vec!["--read.part.path=x", "--read.part.count=1,\"injected\":2"],
        vec!["--read.part.path=x", "--inputs", "{}"],
        vec!["--inputs", "{}", "--read.part.path=x"],
        vec!["--read.part.path=x", "--inputs-file", "missing"],
        vec!["--inputs", "{\"read.part\":{\"path\":\"x\"}}"],
    ] {
        assert!(
            WorkflowArguments::parse(arguments.iter().map(Into::into), &schema).is_err(),
            "{arguments:?}"
        );
    }
    let oversized = format!(
        "--read.part.path={}",
        "x".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES)
    );
    assert!(WorkflowArguments::parse([oversized.into()], &schema).is_err());
    let expanded = format!(
        "--read.part.path={}",
        "\n".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES / 2)
    );
    assert!(WorkflowArguments::parse([expanded.into()], &schema).is_err());
    let padded = format!("[{}]", " ".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES));
    for arguments in [
        vec![
            "--read.part.path=x".into(),
            "--read.part.items".into(),
            padded.clone().into(),
        ],
        vec![
            "--read.part.path=x".into(),
            format!("--read.part.items={padded}").into(),
        ],
    ] {
        assert!(WorkflowArguments::parse(arguments, &schema).is_err());
    }
}

#[test]
fn parameter_sources_are_unambiguous() {
    for arguments in [
        vec!["--inputs"],
        vec!["--inputs", "{}", "--inputs", "{}"],
        vec!["--inputs", "{}", "--inputs-file", "missing"],
        vec!["--unknown"],
        vec!["--inputs", "{\"a\":{},\"a\":{}}"],
    ] {
        assert!(
            WorkflowArguments::parse(
                arguments.iter().map(Into::into),
                &WorkflowInputSchema::default()
            )
            .is_err(),
            "{arguments:?}"
        );
    }
    for flag in ["--validate", "--describe", "--describe-interface"] {
        let error =
            WorkflowArguments::parse([flag.into()], &WorkflowInputSchema::default()).unwrap_err();
        assert!(error.to_string().contains("unknown workflow argument"));
    }
    assert_eq!(
        WorkflowArguments::parse([], &WorkflowInputSchema::default()).unwrap(),
        WorkflowArguments::default()
    );
}
