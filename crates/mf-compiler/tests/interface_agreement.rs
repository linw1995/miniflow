#![cfg(all(unix, feature = "codegen"))]
mod common;

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

fn compile(root: &Path) -> Result<(), mf_compiler::PipelineError> {
    compile_project_with_options(
        &CompileRequest {
            definition: &root.join("workflow.json"),
            output: &root.join("runner"),
            locked: false,
            build_dir: Some(&root.join("build")),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .map(|_| ())
}

#[test]
fn generated_task_and_stream_runners_reject_runtime_contract_changes_before_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes");
    for stream in [false, true] {
        let mut definition = json!({"version":"2026-10-03", "dependencies": {
            "fixture": {"package":"fixture-multi-nodes", "path":fixture}
        }, "nodes":[{"id":"source./~", "kind":"fixture.startup_contract", "config":{"stream":stream}}],
        "outputs":[{"name":"result", "node":"source./~", "port":"value"}]});
        if stream {
            definition["execution"] = json!({"mode":"stream"});
        }
        fs::write(root.path().join("workflow.json"), definition.to_string()).unwrap();
        compile(root.path()).unwrap();
        let runner = root.path().join("runner");
        let marker = root.path().join("executed");
        for mode in ["type", "required", "remove", "add", "stdin", "condition"] {
            let flags = ["--inputs", "{\"source./~\":{\"path./~\":\"ignored\"}}"];
            let output = Command::new(&runner)
                .args(flags)
                .env("MF_FIXTURE_INTERFACE_CHANGE", mode)
                .env("MF_FIXTURE_EXECUTION_MARKER", &marker)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(!output.status.success(), "{mode} was accepted");
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(
                diagnostic.contains("interface drift") && diagnostic.contains("/source.~1~0"),
                "{diagnostic}"
            );
            assert!(!marker.exists(), "{mode} dispatched business execution");
        }
        let output = Command::new(&runner)
            .env("MF_FIXTURE_EXECUTION_MARKER", &marker)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            json!({"result":"executed"})
        );
        assert!(marker.exists());
        fs::remove_file(marker).unwrap();
    }
}
