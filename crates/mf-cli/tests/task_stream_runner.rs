#![cfg(unix)]

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use mf_runtime::{LiveOutputDecoder, LiveOutputKind, WorkflowManifest};
use mf_telemetry::identity::RunId;
use serde_json::json;
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Command, Stdio},
};

fn crates_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn compile(root: &Path, document: serde_json::Value) -> std::path::PathBuf {
    let definition = root.join("flow.json");
    let output = root.join("flow");
    fs::write(&definition, document.to_string()).unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &definition,
            output: &output,
            locked: false,
            build_dir: Some(&root.join("build")),
            support: &SupportPackages::Local {
                crates_dir: crates_dir(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    output
}

fn dependencies() -> serde_json::Value {
    json!({
        "fixture":{"package":"fixture-multi-nodes","path":crates_dir().join("mf-compiler/tests/fixtures/multi-nodes")},
        "core":{"package":"mfn-core","path":crates_dir().join("builtin-nodes/core")}
    })
}

#[test]
fn compiled_finite_live_output_flushes_before_completion_and_preserves_json_mode() {
    let root = tempfile::tempdir().unwrap();
    let gate = root.path().join("release");
    let executable = compile(
        root.path(),
        json!({
            "version":"2026-10-23", "dependencies":dependencies(),
            "nodes":[{"id":"count","kind":"fixture.task_stream","config":{"wait_path":gate}}],
            "outputs":[{"name":"total","node":"count","port":"total"}],
            "stream_outputs":[{"name":"increments","path":["count"],"port":"delta"}]
        }),
    );
    assert!(!gate.exists());
    let legacy = Command::new(&executable)
        .args(["--count.count", "2"])
        .output()
        .unwrap();
    assert!(
        legacy.status.success(),
        "{}",
        String::from_utf8_lossy(&legacy.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&legacy.stdout).unwrap(),
        json!({"total":2})
    );
    let run = RunId::new();
    let mut child = Command::new(&executable)
        .args([
            "--count.count",
            "2",
            "--output-format",
            "events-jsonl",
            "--stream-output",
            "increments",
        ])
        .env("MF_RUN_ID", run.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let manifest = mf_tui::manifest::read_manifest(&executable).unwrap();
    assert!(manifest.supports_live_output());
    let mut decoder =
        LiveOutputDecoder::new(run, &manifest.stream_outputs, Default::default()).unwrap();
    let mut events = Vec::new();
    for _ in 0..2 {
        let mut line = Vec::new();
        output.read_until(b'\n', &mut line).unwrap();
        decoder
            .feed(&line, |event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
    }
    assert!(matches!(events[1].event, LiveOutputKind::Value { .. }));
    assert!(child.try_wait().unwrap().is_none());
    fs::write(&gate, "release").unwrap();
    let mut rest = Vec::new();
    output.read_to_end(&mut rest).unwrap();
    decoder
        .feed(&rest, |event| {
            events.push(event);
            Ok(())
        })
        .unwrap();
    decoder.finish(|_| Ok(())).unwrap();
    let status = child.wait().unwrap();
    let mut errors = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(status.success(), "{errors}");
    assert_eq!(events.len(), 4);
    assert_eq!(decoder.final_outputs().unwrap()["total"].as_u64(), Some(2));
}

#[test]
fn generated_scoped_exports_preserve_full_loop_and_iteration_paths() {
    for mode in ["loop", "parallel"] {
        let root = tempfile::tempdir().unwrap();
        let (body, port) = if mode == "loop" {
            (
                json!({"id":"repeat","kind":"workflow.loop","inputs":{"count":{"from":{"node":"seed","port":"value"}}},"loop":{
                    "variables":[{"name":"count","type":"usize"}],"max_iterations":2,
                    "body":{"nodes":[{"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%loop","port":"count"}}}}]}
                }}),
                "count",
            )
        } else {
            (
                json!({"id":"repeat","kind":"builtin.iteration","inputs":{"items":{"from":{"node":"seed","port":"value"}}},"config":{
                    "mode":"parallel","body":{"nodes":[{"id":"count","kind":"fixture.task_stream","inputs":{"count":{"from":{"node":"%iteration","port":"item"}}}}],"result":{"node":"count","port":"total"}}
                }}),
                "results",
            )
        };
        let executable = compile(
            root.path(),
            json!({
                "version":"2026-10-23","dependencies":dependencies(),
                "nodes":[{"id":"seed","kind":"builtin.constant","config":{"value":if mode=="loop" {json!(1)} else {json!([1,2])}}},body],
                "outputs":[{"name":"result","node":"repeat","port":port}],
                "stream_outputs":[{"name":"increments","path":["repeat","count"],"port":"delta"}]
            }),
        );
        let manifest: WorkflowManifest = mf_tui::manifest::read_manifest(&executable).unwrap();
        assert_eq!(manifest.stream_outputs.len(), 1);
        let run = RunId::new();
        let output = Command::new(&executable)
            .args(["--output-format", "events-jsonl"])
            .env("MF_RUN_ID", run.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut decoder =
            LiveOutputDecoder::new(run, &manifest.stream_outputs, Default::default()).unwrap();
        let mut events = Vec::new();
        decoder
            .feed(&output.stdout, |event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
        decoder.finish(|_| Ok(())).unwrap();
        let opens = events
            .iter()
            .filter(|event| matches!(event.event, LiveOutputKind::Open))
            .collect::<Vec<_>>();
        assert_eq!(opens.len(), 2);
        assert!(
            opens
                .iter()
                .all(|event| event.source == vec!["repeat".into(), "count".into()])
        );
        assert_ne!(opens[0].invocation.id, opens[1].invocation.id);
        assert!(opens.iter().all(|event| event.invocation.path.len() == 1));
    }
}

#[test]
fn native_stream_combined_targets_remain_isolated_terminal_tasks() {
    let root = tempfile::tempdir().unwrap();
    let gate = root.path().join("never-released");
    let executable = compile(
        root.path(),
        json!({
            "version":"2026-10-23", "dependencies":dependencies(), "execution":{"mode":"stream"},
            "nodes":[
                {"id":"count","kind":"fixture.task_stream","config":{"wait_path":gate}},
                {"id":"consume","kind":"builtin.identity","inputs":{"input":{"from":{"node":"count","port":"delta"}}}}
            ],
            "outputs":[{"name":"total","node":"count","port":"total"}],
            "stream_outputs":[{"name":"increments","path":["count"],"port":"delta"}]
        }),
    );
    let output = Command::new(&executable)
        .args(["--node", "count", "--count.count", "2"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"total":2})
    );
    assert!(!gate.exists());
    let output = Command::new(&executable)
        .args(["--count.count", "2", "--output-format", "events-jsonl"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires finite execution"));
}

#[test]
fn compiled_live_contract_drift_and_unknown_subscriptions_fail_before_business_work() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    let executable = compile(
        root.path(),
        json!({
            "version":"2026-10-23","dependencies":dependencies(),
            "nodes":[{"id":"count","kind":"fixture.task_stream","config":{"marker":marker}}],
            "outputs":[{"name":"total","node":"count","port":"total"}],
            "stream_outputs":[{"name":"increments","path":["count"],"port":"delta"}]
        }),
    );
    assert!(!marker.exists());
    let output = Command::new(&executable)
        .args(["--count.count", "1", "--output-format", "events-jsonl"])
        .env("MF_FIXTURE_STREAM_CONTRACT_CHANGE", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("/stream_outputs"));
    assert!(!marker.exists());
    let output = Command::new(&executable)
        .args([
            "--count.count",
            "1",
            "--output-format",
            "events-jsonl",
            "--stream-output",
            "missing",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not declared"));
    assert!(!marker.exists());
}
