use mf_runtime::{NodeTarget, RunnerInvocation, WorkflowInputSchema};
use serde_json::json;
use std::ffi::OsString;

fn target() -> NodeTarget {
    let id = mf_telemetry::identity::WorkflowId::from_definition(&json!({}), &[]).unwrap();
    serde_json::from_value(json!({
        "id":"copy.part", "index":0, "kind":"Task", "outputs":{}, "context":{},
        "original_workflow_id":id,
        "description":{"version":"2026-09-29", "workflow_id":id, "nodes":[{"id":"copy.part", "kind":"test.task"}], "data_edges":[], "control_edges":[], "execution_order":["copy.part"]},
        "inputs":{"inputs":{"copy.part":{
            "text":{"type":"string", "required":true}, "count":{"type":"int", "required":false},
            "label":{"type":{"nullable":"string"}, "required":false}
        }}, "stdin":{}}
    })).unwrap()
}
fn parse(
    args: &[&str],
    target: &NodeTarget,
) -> Result<RunnerInvocation, mf_runtime::RunnerArgumentError> {
    RunnerInvocation::parse(
        args.iter().map(OsString::from),
        &WorkflowInputSchema::default(),
        std::slice::from_ref(target),
    )
}

#[test]
fn selection_preserves_values_types_presence_and_normal_arguments() {
    let target = target();
    let value = parse(
        &[
            "--copy.part.text=--node",
            "--node=copy.part",
            "--copy.part.count",
            "42",
            "--copy.part.label=null",
        ],
        &target,
    )
    .unwrap();
    assert_eq!(value.node.as_deref(), Some("copy.part"));
    assert_eq!(
        serde_json::to_value(value.inputs).unwrap(),
        json!({"copy.part.text":"--node", "copy.part.count":42, "copy.part.label":null})
    );
    for args in [
        vec![
            "--node",
            "copy.part",
            "--inputs",
            "{\"copy.part.text\":\"hello\"}",
        ],
        vec!["--copy.part.text", "hello", "--node", "copy.part"],
    ] {
        let value = parse(&args, &target).unwrap();
        assert_eq!(value.inputs.0.len(), 1);
    }
    for (args, expected, text) in [
        (
            vec!["--verbose", "--node=copy.part", "--copy.part.text", "-vv"],
            1,
            "-vv",
        ),
        (
            vec!["--node=copy.part", "-vv", "--copy.part.text=--verbose"],
            2,
            "--verbose",
        ),
        (
            vec![
                "--node=copy.part",
                "--copy.part.text",
                "-v",
                "-vvv",
                "--verbose",
            ],
            3,
            "-v",
        ),
    ] {
        let value = parse(&args, &target).unwrap();
        assert_eq!(value.verbose, expected);
        assert_eq!(value.inputs.0["copy.part.text"], json!(text));
    }
    let normal = RunnerInvocation::parse(
        ["--copy.part.text", "hello", "-vv"].map(Into::into),
        &target.inputs,
        &[],
    )
    .unwrap();
    assert!(normal.node.is_none());
    assert_eq!(normal.verbose, 2);
    assert_eq!(normal.inputs.0["copy.part.text"], json!("hello"));
}

#[test]
fn rejects_invalid_targets_inputs_and_context_controls() {
    let target = target();
    for args in [
        vec!["--node"],
        vec!["--node="],
        vec!["--node", "unknown"],
        vec!["--node", "copy.part", "--node=copy.part"],
        vec!["--node", "copy.part"],
        vec!["--node=copy.part", "--other.path=x"],
        vec![
            "--node=copy.part",
            "--copy.part.text=x",
            "--copy.part.text=y",
        ],
        vec![
            "--node=copy.part",
            "--copy.part.text=x",
            "--copy.part.count=null",
        ],
        vec!["--node=copy.part", "--copy.part.text=x", "--inputs", "{}"],
        vec![
            "--node=copy.part",
            "--inputs",
            "{\"copy.part.text\":\"x\",\"copy.part.text\":\"y\"}",
        ],
        vec!["--context", "{}"],
        vec![
            "--node=copy.part",
            "--copy.part.text=x",
            "--context",
            "{}",
            "--context-file",
            "file",
        ],
        vec!["--node=copy.part", "--copy.part.text=x", "--context", "[]"],
        vec![
            "--node=copy.part",
            "--copy.part.text=x",
            "--context",
            "{\"source.value\":1}",
        ],
    ] {
        assert!(parse(&args, &target).is_err(), "{args:?}");
    }
    let args = [OsString::from("--node"), {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![0xff])
        }
        #[cfg(not(unix))]
        {
            OsString::new()
        }
    }];
    assert!(RunnerInvocation::parse(args, &target.inputs, std::slice::from_ref(&target)).is_err());
}

#[test]
fn validates_context_values_and_preserves_typed_error_sources() {
    use std::error::Error;
    let mut target = target();
    target
        .context
        .insert("source.part.value".into(), mf_runtime::ValueType::Int64);
    let base = ["--node", "copy.part", "--copy.part.text=x"];
    for context in [
        "{}",
        "{\"other.value\":42}",
        "{\"source.part.value\":null}",
        "{\"source.part.value\":\"x\"}",
        "{\"source.part.value\":1,\"source.part.value\":2}",
    ] {
        let args = [base.as_slice(), &["--context", context]].concat();
        assert!(parse(&args, &target).is_err(), "{context}");
    }
    let value = parse(
        &[
            "--node=copy.part",
            "--copy.part.text=x",
            "--context",
            "{\"source.part.value\":42}",
        ],
        &target,
    )
    .unwrap();
    assert_eq!(value.context.0["source.part.value"], json!(42));
    let error = parse(
        &[
            "--node=copy.part",
            "--copy.part.text=x",
            "--context",
            "{\"source.part.value\":\"x\"}",
        ],
        &target,
    )
    .unwrap_err();
    let mut source = error.source();
    let mut typed = false;
    while let Some(cause) = source {
        typed |= cause.is::<mf_runtime::TypeMismatch>();
        source = cause.source();
    }
    assert!(typed);
}

#[test]
fn context_files_are_bounded_regular_and_read_once() {
    let target = target();
    let root = tempfile::tempdir_in(".").unwrap();
    let path = root.path().join("context.json");
    std::fs::write(&path, "{}").unwrap();
    let value = parse(
        &[
            "--node=copy.part",
            "--copy.part.text=x",
            "--context-file",
            path.to_str().unwrap(),
        ],
        &target,
    )
    .unwrap();
    std::fs::write(&path, "invalid").unwrap();
    assert!(value.context.0.is_empty());
    assert!(
        parse(
            &[
                "--node=copy.part",
                "--copy.part.text=x",
                "--context-file",
                path.to_str().unwrap()
            ],
            &target
        )
        .is_err()
    );
    for file in [root.path().to_owned(), root.path().join("missing")] {
        assert!(
            parse(
                &[
                    "--node=copy.part",
                    "--copy.part.text=x",
                    "--context-file",
                    file.to_str().unwrap()
                ],
                &target
            )
            .is_err()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let fifo = root.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(
            parse(
                &[
                    "--node=copy.part",
                    "--copy.part.text=x",
                    "--context-file",
                    fifo.to_str().unwrap()
                ],
                &target
            )
            .unwrap_err()
            .to_string()
            .contains("regular context file")
        );
    }
    std::fs::write(&path, " ".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES + 1)).unwrap();
    assert!(
        parse(
            &[
                "--node=copy.part",
                "--copy.part.text=x",
                "--context-file",
                path.to_str().unwrap()
            ],
            &target
        )
        .is_err()
    );
}

#[test]
fn combined_canonical_payload_has_a_checked_boundary() {
    let mut target = target();
    target
        .context
        .insert("source.value".into(), mf_runtime::ValueType::String);
    let empty_size =
        serde_json::to_vec(&json!({"inputs":{"copy.part.text":""}, "context":{"source.value":""}}))
            .unwrap()
            .len();
    let half = (mf_runtime::MAX_WORKFLOW_INPUT_BYTES - empty_size) / 2;
    let text = "x".repeat(half);
    let context = json!({"source.value":"x".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES - empty_size - half)}).to_string();
    let value = format!("--copy.part.text={text}");
    assert!(
        parse(
            &["--node=copy.part", &value, "--context", &context],
            &target
        )
        .is_ok()
    );
    assert!(
        parse(
            &[
                "--node=copy.part",
                &format!("{value}x"),
                "--context",
                &context
            ],
            &target
        )
        .is_err()
    );
    assert!(
        parse(
            &[
                "--node=copy.part",
                &format!("--copy.part.text={}", "\n".repeat(half)),
                "--context",
                &context
            ],
            &target
        )
        .is_err()
    );
}
