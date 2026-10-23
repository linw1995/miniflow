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

#[test]
fn answer_and_title_share_one_result_while_only_answer_is_streamed() {
    use mfn_llm::test_support::{Reply, Server};
    let text = json!({"id":"answer", "object":"chat.completion.chunk", "created":0, "model":"example-model", "choices":[{"index":0,"delta":{"content":"Live answer"},"finish_reason":null}]});
    let finish = json!({"id":"answer", "object":"chat.completion.chunk", "created":0, "model":"example-model", "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
    let mut streamed = Reply::text("unused");
    streamed
        .headers
        .push(("Content-Type".into(), "text/event-stream".into()));
    streamed.body = format!("data: {text}\n\ndata: {finish}\n\ndata: [DONE]\n\n");
    let server = Server::start(vec![
        streamed,
        Reply::text("Generated title"),
        Reply::text("Live answer"),
        Reply::text("Generated title"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let mut definition: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/chat-title-flow.json")).unwrap();
    for (alias, path) in [("core", "core"), ("code", "code"), ("llm", "llm")] {
        definition["dependencies"][alias]["path"] =
            json!(crates_dir().join("builtin-nodes").join(path));
    }
    let executable = compile(root.path(), definition);
    let manifest = mf_tui::manifest::read_manifest(&executable).unwrap();
    assert_eq!(manifest.stream_outputs.len(), 1);
    let mut canonical = None;
    for live in [true, false] {
        let run = RunId::new();
        let mut command = Command::new(&executable);
        command
            .args([
                "--input.prompt",
                "Hello",
                "--input.base_url",
                &server.base_url,
            ])
            .env("MODEL_API_KEY", "synthetic-key")
            .env("MF_RUN_ID", run.to_string())
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        if live {
            command.args([
                "--output-format",
                "events-jsonl",
                "--stream-output",
                "answer",
            ]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result = if live {
            let mut decoder =
                LiveOutputDecoder::new(run, &manifest.stream_outputs, Default::default()).unwrap();
            let mut values = 0;
            decoder
                .feed(&output.stdout, |event| {
                    assert_eq!(event.export, "answer");
                    if let LiveOutputKind::Value { value } = event.event {
                        assert_eq!(value["text"].as_str(), Some("Live answer"));
                        values += 1;
                    }
                    Ok(())
                })
                .unwrap();
            decoder.finish(|_| Ok(())).unwrap();
            assert_eq!(values, 1);
            serde_json::to_value(decoder.take_final_outputs().unwrap()).unwrap()
        } else {
            serde_json::from_slice(&output.stdout).unwrap()
        };
        assert_eq!(result["text"], "Live answer");
        assert_eq!(result["title"], "Generated title");
        assert_eq!(result["usages"].as_array().unwrap().len(), 2);
        if live {
            canonical = Some(result["text"].clone());
        } else {
            assert_eq!(canonical.as_ref().unwrap(), &result["text"]);
        }
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].body["stream"], true);
    for request in &requests[1..] {
        assert_ne!(request.body["stream"], true);
    }
}
