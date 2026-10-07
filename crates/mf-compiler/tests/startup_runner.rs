#![cfg(unix)]
mod common;
use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn standalone_sources_describe_parameters_and_run_without_stdin_or_build_inputs() {
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("workflow.json");
    let executable = root.path().join("workflow");
    let build = root.path().join("build");
    let data = root.path().join("not-created-during-preflight.txt");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes");
    fs::write(&definition, json!({"version":"2026-10-03", "execution":{"mode":"stream", "limits":{"max_pending_messages":2, "workers":1}},
        "dependencies":{"fixture":{"package":"fixture-multi-nodes", "path":fixture}},
        "nodes":[{"id":"read", "kind":"fixture.read_lines"}],
        "outputs":[{"name":"line", "node":"read", "port":"line"}]}).to_string()).unwrap();
    let support = SupportPackages::Local {
        crates_dir: common::crates_dir(),
    };
    compile_project_with_options(
        &CompileRequest {
            definition: &definition,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &support,
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    let run = |arguments: &[&str]| {
        Command::new(&executable)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    assert!(!data.exists());
    for arguments in [
        vec![],
        vec!["--inputs", "{}"],
        vec!["--inputs", "{\"read\":{\"path\":42}}"],
    ] {
        assert!(!run(&arguments).status.success());
    }
    let lines: Vec<_> = (0..200).map(|line| format!("line {line}")).collect();
    fs::write(&data, lines.join("\n")).unwrap();
    let values = json!({"read":{"path":data}}).to_string();
    let input_file = root.path().join("parameters.json");
    fs::write(&input_file, &values).unwrap();
    fs::remove_file(&definition).unwrap();
    fs::remove_dir_all(&build).unwrap();
    for arguments in [
        vec!["--inputs", values.as_str()],
        vec!["--inputs-file", input_file.to_str().unwrap()],
    ] {
        let output = run(&arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual: Vec<Value> = output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(
            actual,
            lines
                .iter()
                .map(|line| json!({"line":line}))
                .collect::<Vec<_>>()
        );
    }
}
