#![cfg(unix)]

use mf_compiler::{
    CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options, plan_definition,
    resolve_project, write_dependency_project_with_options,
};
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
    for scenario in ["task", "stream", "nested", "ordered"] {
        let root = tempfile::tempdir().unwrap();
        let definition_path = root.path().join("workflow.json");
        let installed = root.path().join("runner");
        let standalone = root.path().join("standalone");
        let build = root.path().join("build");
        let data = root.path().join("runtime-input.txt");
        let (mut definition, arguments, expected) = match scenario {
            "ordered" => (
                json!({"version":"2026-10-19","nodes":[
                    {"id":"z_start","kind":"builtin.constant","config":{"value":0}},
                    {"id":"a_end","kind":"builtin.constant","config":{"value":42}}
                ],"edges":[{"from_node":"z_start","to_node":"a_end"}],"outputs":[{"name":"result","node":"a_end","port":"value"}]}),
                json!({}),
                json!({"result":42}),
            ),
            "task" => (
                json!({"version":"2026-10-03", "nodes":[{"id":"identity", "kind":"builtin.identity"}],
                "outputs":[{"name":"result", "node":"identity", "port":"value"}]}),
                json!({"identity.input":27}),
                json!({"result":27}),
            ),
            "stream" => (
                json!({"version":"2026-10-03", "execution":{"mode":"stream"},
                "nodes":[{"id":"read", "kind":"builtin.readline"}],
                "outputs":[{"name":"result", "node":"read", "port":"line"}]}),
                json!({"read.path":data}),
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
        let manifest = read_manifest(&installed).unwrap();
        if scenario == "ordered" {
            assert_eq!(manifest.description().unwrap().order_edges.len(), 1);
            assert_eq!(
                manifest.description().unwrap().event_schema_version(),
                mf_telemetry::SUCCESS_EVENT_SCHEMA_VERSION
            );
        }
        if scenario == "nested" {
            assert_eq!(manifest.description().unwrap().loop_bodies.len(), 1);
        }
        fs::rename(&installed, &standalone).unwrap();
        fs::remove_file(&definition_path).unwrap();
        fs::remove_file(definition_path.with_extension("lock")).unwrap();
        fs::remove_dir_all(&build).unwrap();
        assert_eq!(read_manifest(&standalone).unwrap(), manifest);
        for flag in ["--describe", "--describe-interface", "--validate"] {
            let output = Command::new(&standalone)
                .arg(flag)
                .env("PATH", "")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("unexpected argument"), "{stderr}");
            assert!(stderr.contains(flag), "{stderr}");
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
        assert!(output.stderr.is_empty());
        for (flag, through_cli) in [("-v", false), ("-vv", true), ("-vvv", false)] {
            let mut command = if through_cli {
                let mut command = Command::new(env!("CARGO_BIN_EXE_mf"));
                command.arg("run").arg(&standalone);
                command
            } else {
                Command::new(&standalone)
            };
            let verbose = command
                .args([flag, "--inputs", &arguments.to_string()])
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&verbose.stderr);
            assert!(verbose.status.success(), "{stderr}");
            assert_eq!(verbose.stdout, output.stdout);
            if flag == "-vvv" {
                let records: Vec<mf_telemetry::wire::WireRecord> = stderr
                    .lines()
                    .map(|line| {
                        serde_json::from_str(line.strip_prefix("[verbose] ").unwrap()).unwrap()
                    })
                    .collect();
                assert_eq!(records.first().unwrap().event_name, "mf.workflow.started");
                assert_eq!(records.last().unwrap().event_name, "mf.workflow.finished");
                assert!(
                    records
                        .iter()
                        .any(|record| record.event_name == "mf.node.started")
                );
                assert!(records.iter().all(|record| record.trace_context.is_none()));
                if scenario == "nested" {
                    assert!(
                        records
                            .iter()
                            .any(|record| record.event_name == "mf.loop.pass.started")
                    );
                }
            } else {
                assert!(stderr.contains("workflow started"), "{stderr}");
                assert!(stderr.contains("workflow succeeded"), "{stderr}");
                assert!(stderr.contains("node "), "{stderr}");
                assert_eq!(stderr.contains(" | event: "), flag == "-vv");
                if scenario == "nested" {
                    assert!(stderr.contains("loop pass started"), "{stderr}");
                }
            }
        }
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

#[test]
fn release_lto_and_strip_preserve_standalone_factory_free_manifest() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("build");
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let definition = serde_json::from_value(json!({
        "version":"2026-09-26", "dependencies": {"fixture": {
            "package":"fixture-multi-nodes", "path":crates.join("mf-compiler/tests/fixtures/multi-nodes"),
            "features":["double"], "default-features":false
        }},
        "nodes":[{"id":"a", "kind":"fixture.source", "config":{"print":true}}, {"id":"b", "kind":"fixture.echo"}],
        "edges":[{"from_node":"a","from_output":"value","to_node":"b","to_input":"input"}],
        "outputs":[{"name":"result","node":"b","port":"value"}]
    })).unwrap();
    let plan = plan_definition(&definition).unwrap();
    for telemetry in [false, true] {
        write_dependency_project_with_options(
            &project,
            &plan,
            &SupportPackages::Local {
                crates_dir: crates.to_owned(),
            },
            &RunnerOptions { telemetry },
        )
        .unwrap();
        let manifest_path = project.join("Cargo.toml");
        let mut cargo_manifest = fs::read_to_string(&manifest_path).unwrap();
        cargo_manifest.push_str("\n[profile.release]\nlto = true\ncodegen-units = 1\n");
        fs::write(manifest_path, cargo_manifest).unwrap();
        resolve_project(&project, &root.path().join("flow.lock"), false).unwrap();
        let output = mf_compiler::cargo_command(&project)
            // This probe covers optimized artifacts even when behavior tests build cheaply.
            .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3")
            .args(["build", "--offline", "--release", "--locked"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let runner = root.path().join("standalone");
        let target = std::env::var_os("MF_TEST_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| project.join("target"));
        fs::copy(target.join("release/mf-generated-workflow"), &runner).unwrap();
        let before = read_manifest(&runner).unwrap();
        assert_eq!(before.definition, plan.definition);
        assert_eq!(
            before.node_definition("a").unwrap().config,
            json!({"print":true})
        );
        let output = Command::new("strip").arg(&runner).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(read_manifest(&runner).unwrap(), before);
    }
}
