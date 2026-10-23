use mf_runtime::{WorkflowArguments, WorkflowInputSchema};
use serde_json::json;

#[test]
fn live_negotiation_requires_a_capable_manifest_and_declared_exports() {
    use mf_runtime::{
        RunnerInvocation, RunnerOutputFormat, WorkflowManifest, WorkflowManifestVersion,
    };
    let mut manifest = WorkflowManifest {
        version: WorkflowManifestVersion::V2026_10_16,
        definition: serde_json::from_value(json!({
            "version": "2026-10-23", "dependencies": {}, "nodes": []
        }))
        .unwrap(),
        execution_order: Vec::new(),
        loop_orders: Vec::new(),
        input_schema: WorkflowInputSchema::default(),
        stream_outputs: Vec::new(),
    };
    assert!(
        RunnerInvocation::default()
            .validate_output(&manifest)
            .is_ok()
    );
    let mut invocation = RunnerInvocation {
        output_format: RunnerOutputFormat::EventsJsonl,
        stream_outputs: vec!["answer".into()],
        ..Default::default()
    };
    assert!(invocation.validate_output(&manifest).is_err());
    manifest.version = WorkflowManifestVersion::V2026_10_23;
    assert!(invocation.validate_output(&manifest).is_err());
    manifest
        .stream_outputs
        .push(mf_runtime::StreamOutputContract {
            name: "answer".into(),
            path: vec!["producer".into()],
            port: "delta".into(),
            value_type: mf_runtime::ValueType::String,
            required: false,
        });
    invocation.validate_output(&manifest).unwrap();
    invocation.stream_outputs.push("private".into());
    assert!(invocation.validate_output(&manifest).is_err());
}

#[test]
fn invocation_selects_finite_live_protocol_without_consuming_workflow_values() {
    let invocation = mf_runtime::RunnerInvocation::parse(
        [
            "--output-format",
            "events-jsonl",
            "--stream-output",
            "answer",
        ]
        .map(Into::into),
        &WorkflowInputSchema::default(),
        &[],
    )
    .unwrap();
    assert_eq!(
        invocation.output_format,
        mf_runtime::RunnerOutputFormat::EventsJsonl
    );
    assert_eq!(invocation.stream_outputs, vec!["answer"]);
    for flags in [
        vec!["--stream-output", "answer"],
        vec!["--output-format", "json", "--stream-output", "answer"],
        vec![
            "--output-format",
            "events-jsonl",
            "--stream-output",
            "answer",
            "--stream-output",
            "answer",
        ],
        vec!["--output-format", "unknown"],
    ] {
        assert!(
            mf_runtime::RunnerInvocation::parse(
                flags.into_iter().map(Into::into),
                &WorkflowInputSchema::default(),
                &[]
            )
            .is_err()
        );
    }
    let invocation = mf_runtime::RunnerInvocation::parse(
        [
            "--read.part.path=--output-format",
            "--output-format",
            "events-jsonl",
        ]
        .map(Into::into),
        &schema(),
        &[],
    )
    .unwrap();
    assert_eq!(
        invocation.inputs.0["read.part.path"].as_str(),
        Some("--output-format")
    );
}

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
        (vec!["--"], 0),
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
        vec![
            "--read.part.path=x",
            "--inputs",
            r#"{"read.part.path":"y"}"#,
        ],
        vec![
            "--inputs",
            r#"{"read.part.path":"y"}"#,
            "--read.part.path=x",
        ],
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
        let mf_runtime::RunnerArgumentError::Cli { source } = error else {
            panic!("expected a clap error");
        };
        assert_eq!(source.kind(), clap::error::ErrorKind::UnknownArgument);
    }
    assert_eq!(
        WorkflowArguments::parse([], &WorkflowInputSchema::default()).unwrap(),
        WorkflowArguments::default()
    );
}

#[test]
fn long_options_remain_flags_and_short_option_values_remain_literal() {
    for flag in ["--verbose", "--inputs", "--help"] {
        assert!(
            mf_runtime::RunnerArguments::parse(
                ["--read.part.path", flag].map(Into::into),
                &schema(),
            )
            .is_err(),
            "{flag}",
        );
    }
    for value in ["-h", "-", "-42"] {
        let options = mf_runtime::RunnerArguments::parse(
            ["--read.part.path", value, "-v"].map(Into::into),
            &schema(),
        )
        .unwrap();
        assert_eq!(options.inputs.0["read.part.path"], json!(value));
        assert_eq!(options.verbose, 1);
    }
}

#[test]
fn inline_transports_preserve_exclusivity_and_file_limits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("inputs.json");
    std::fs::write(&path, r#"{"read.part.path":"data.txt"}"#).unwrap();
    for argument in [
        r#"--inputs={"read.part.path":"data.txt"}"#.to_owned(),
        format!("--inputs-file={}", path.display()),
    ] {
        let options = mf_runtime::RunnerArguments::parse([argument.into()], &schema()).unwrap();
        assert_eq!(options.inputs.0["read.part.path"], json!("data.txt"));
    }
    for args in [
        vec!["--inputs={}", "--inputs={}"],
        vec!["--inputs={}", "--inputs-file=missing"],
        vec![r#"--inputs={"read.part.path":"y"}"#, "--read.part.path=x"],
    ] {
        assert!(
            mf_runtime::RunnerArguments::parse(args.into_iter().map(Into::into), &schema())
                .is_err()
        );
    }
    std::fs::write(&path, " ".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES + 1)).unwrap();
    assert!(
        mf_runtime::RunnerArguments::parse(
            [format!("--inputs-file={}", path.display()).into()],
            &schema(),
        )
        .is_err()
    );
}

#[test]
fn help_describes_dynamic_inputs_without_required_input_validation() {
    for flag in ["--help", "-h"] {
        let error = mf_runtime::RunnerArguments::parse([flag.into()], &schema()).unwrap_err();
        let mf_runtime::RunnerArgumentError::Cli { source } = error else {
            panic!("expected a clap help response");
        };
        assert_eq!(source.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = source.to_string();
        assert!(help.contains("--read.part.path <VALUE>"));
        assert!(help.contains("string (required)"));
        assert!(help.contains("list<int64> (optional)"));
        assert!(help.contains("--inputs-file <PATH>"));
        assert!(help.contains("--verbose"));
    }
}
