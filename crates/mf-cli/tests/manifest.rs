#![cfg(unix)]

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use mf_tui::manifest::read_manifest;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn moved_task_stream_and_nested_runners_keep_manifest_and_execution_contracts() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let support = SupportPackages::Local {
        crates_dir: crates.to_owned(),
    };
    for scenario in ["task", "stream", "nested"] {
        let root = tempfile::tempdir().unwrap();
        let definition_path = root.path().join("workflow.json");
        let installed = root.path().join("runner");
        let standalone = root.path().join("standalone");
        let build = root.path().join("build");
        let data = root.path().join("runtime-input.txt");
        let (mut definition, arguments, expected) = match scenario {
            "task" => (
                json!({"version":"2026-10-03", "nodes":[{"id":"identity", "kind":"builtin.identity"}],
                "outputs":[{"name":"result", "node":"identity", "port":"value"}]}),
                json!({"identity":{"input":27}}),
                json!({"result":27}),
            ),
            "stream" => (
                json!({"version":"2026-10-03", "execution":{"mode":"stream"},
                "nodes":[{"id":"read", "kind":"builtin.readline"}],
                "outputs":[{"name":"result", "node":"read", "port":"line"}]}),
                json!({"read":{"path":data}}),
                json!({"result":"hello"}),
            ),
            _ => (
                serde_json::from_str::<Value>(include_str!("../../../examples/loop.json")).unwrap(),
                json!({}),
                json!({"count":3}),
            ),
        };
        definition["dependencies"] = json!({
            "core":{"package":"mfn-core", "path":crates.join("builtin-nodes/core")},
            "code":{"package":"mfn-code", "path":crates.join("builtin-nodes/code")}
        });
        fs::write(&definition_path, definition.to_string()).unwrap();
        compile_project_with_options(
            &CompileRequest {
                definition: &definition_path,
                output: &installed,
                locked: false,
                build_dir: Some(&build),
                support: &support,
            },
            &RunnerOptions { telemetry: false },
        )
        .unwrap();
        let manifest = read_manifest(&installed).unwrap().unwrap();
        if scenario == "nested" {
            assert_eq!(manifest.description.loop_bodies.len(), 1);
        }
        fs::rename(&installed, &standalone).unwrap();
        fs::remove_file(&definition_path).unwrap();
        fs::remove_file(definition_path.with_extension("lock")).unwrap();
        fs::remove_dir_all(&build).unwrap();
        assert_eq!(read_manifest(&standalone).unwrap().unwrap(), manifest);
        for flag in ["--describe", "--describe-interface", "--validate"] {
            let output = Command::new(&standalone)
                .arg(flag)
                .env("PATH", "")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            match flag {
                "--describe" => assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    serde_json::to_value(&manifest.description).unwrap()
                ),
                "--describe-interface" => assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    serde_json::to_value(&manifest.interface).unwrap()
                ),
                _ => {}
            }
        }
        assert!(!data.exists());
        fs::write(&data, "hello\n").unwrap();
        let output = Command::new(&standalone)
            .args(["--inputs", &arguments.to_string()])
            .env("PATH", "")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            expected
        );
        let invalid = Command::new(&standalone)
            .args(["--inputs", "{\"unknown\":{}}"])
            .env("PATH", "")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!invalid.status.success());
        assert!(invalid.stdout.is_empty());
    }
}
