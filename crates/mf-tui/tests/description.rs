#![cfg(unix)]

use mf_telemetry::{
    description::{WorkflowDescription, WorkflowDescriptionVersion},
    identity::WorkflowId,
};
use mf_tui::description::{
    DescriptionError, DescriptionLimits, describe_executable, describe_executable_with_limits,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn create_runner(script: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runner");
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    (directory, path)
}

fn sample() -> (String, WorkflowDescription) {
    let description = WorkflowDescription {
        version: WorkflowDescriptionVersion::CURRENT,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
        nodes: vec![],
        data_edges: vec![],
        control_edges: vec![],
        execution_order: vec![],
    };
    (
        String::from_utf8(description.to_json().unwrap()).unwrap(),
        description,
    )
}

fn limits() -> DescriptionLimits {
    DescriptionLimits {
        timeout: Duration::from_millis(500),
        drain_timeout: Duration::from_millis(100),
        max_bytes: 4096,
        max_diagnostic_bytes: 128,
    }
}

#[test]
fn accepts_one_complete_document_despite_noisy_stderr() {
    let (json, expected) = sample();
    let (_root, runner) = create_runner(&format!(
        "test \"$1\" = --describe || exit 2\nprintf '%s\\n' '{{\"version\":\"2099-01-01\"}}' >&2\nprintf '%s\\n' '{json}'\nprintf 'diagnostic tail\\n' >&2",
    ));
    assert_eq!(describe_executable(&runner).unwrap(), expected);
}

#[test]
fn rejects_unsupported_incomplete_and_multiple_documents() {
    let (json, _) = sample();
    for script in [
        format!(
            "printf '%s\\n' '{}'",
            json.replace("\"version\":\"2026-09-27\"", "\"version\":\"2026-09-26\"")
        ),
        "printf '{\"version\":\\n'".into(),
        format!("printf '%s\\n%s\\n' '{json}' '{{}}'"),
    ] {
        let (_root, runner) = create_runner(&script);
        assert!(matches!(
            describe_executable(&runner),
            Err(DescriptionError::Invalid { .. })
        ));
    }
    let (_root, runner) = create_runner(&format!("printf '%s' '{json}'"));
    assert!(matches!(
        describe_executable(&runner),
        Err(DescriptionError::MissingTerminator)
    ));
}

#[test]
fn bounds_output_and_diagnostic_history_without_blocking_pipes() {
    let (_root, runner) = create_runner("yes x | head -c 131072");
    assert!(matches!(
        describe_executable_with_limits(&runner, limits()),
        Err(DescriptionError::TooLarge { .. })
    ));
    let (_root, runner) =
        create_runner("yes x | head -c 131072 >&2\nprintf '{\"broken\":true}\\n'\nexit 7");
    let error = describe_executable_with_limits(&runner, limits()).unwrap_err();
    match error {
        DescriptionError::Exit { diagnostics, .. } => {
            assert!(diagnostics.contains("dropped") && diagnostics.len() < 256)
        }
        other => panic!("expected bounded diagnostics, got {other}"),
    }
}

#[test]
fn times_out_and_closes_pipes_held_by_descendants() {
    let (_root, runner) = create_runner("sleep 60");
    let started = Instant::now();
    assert!(matches!(
        describe_executable_with_limits(&runner, limits()),
        Err(DescriptionError::Timeout { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(3));

    let (json, _) = sample();
    let (_root, runner) = create_runner(&format!(
        "printf '%s\\n' '{json}'\npython3 -c 'import os,time; p=os.fork(); os._exit(0) if p else time.sleep(60)'",
    ));
    let started = Instant::now();
    assert!(matches!(
        describe_executable_with_limits(&runner, limits()),
        Err(DescriptionError::OpenPipe)
    ));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn reports_nonzero_exits_and_missing_runners_without_execution() {
    let (_root, runner) = create_runner("printf 'usage: workflow [--validate]\\n' >&2\nexit 2");
    let error = describe_executable(&runner).unwrap_err();
    assert!(matches!(error, DescriptionError::Exit { .. }));
    assert!(error.to_string().contains("usage: workflow [--validate]"));
    assert!(error.to_string().contains("recompile"));
    let missing = runner.with_file_name("missing-runner");
    assert!(matches!(
        describe_executable(&missing),
        Err(DescriptionError::Spawn { .. })
    ));
}
