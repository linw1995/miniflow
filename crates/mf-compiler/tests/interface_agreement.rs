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
fn validation_drift_child() {
    if let Some(root) = std::env::var_os("MF_FIXTURE_DRIFT_ROOT") {
        assert!(compile(Path::new(&root)).is_err());
    }
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
            for flags in [
                vec!["--validate"],
                vec!["--inputs", "{\"source./~\":{\"path./~\":\"ignored\"}}"],
            ] {
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
        }
        for flag in ["--describe", "--describe-interface"] {
            let output = Command::new(&runner)
                .arg(flag)
                .env("MF_FIXTURE_INTERFACE_CHANGE", "initialization")
                .output()
                .unwrap();
            assert!(output.status.success());
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
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
        let installed = fs::read(&runner).unwrap();
        let lock = fs::read(root.path().join("workflow.lock")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "validation_drift_child"])
            .env("MF_FIXTURE_DRIFT_ROOT", root.path())
            .env("MF_FIXTURE_VALIDATE_DRIFT", "1")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(fs::read(&runner).unwrap(), installed);
        assert_eq!(fs::read(root.path().join("workflow.lock")).unwrap(), lock);
    }
}
