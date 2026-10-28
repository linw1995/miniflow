#![cfg(unix)]

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
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

fn count_flow(streaming: bool, gate: Option<&Path>, marker: Option<&Path>) -> serde_json::Value {
    let mut document = json!({
        "version":"2026-10-23", "dependencies":dependencies(),
        "nodes":[{"id":"count","kind":"fixture.task_stream","config":{}}],
        "outputs":[{"name":"total","node":"count","port":"total"}]
    });
    if streaming {
        document["outputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"increment","node":"count","port":"delta"}));
    }
    if let Some(gate) = gate {
        document["nodes"][0]["config"]["wait_path"] = json!(gate);
    }
    if let Some(marker) = marker {
        document["nodes"][0]["config"]["marker"] = json!(marker);
    }
    document
}

#[test]
fn compiled_native_stream_delivers_increments_before_the_terminal_result() {
    let root = tempfile::tempdir().unwrap();
    let gate = root.path().join("release");
    let executable = compile(root.path(), count_flow(true, Some(&gate), None));
    let mut child = Command::new(&executable)
        .args(["--count.count", "3"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let first: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(first, json!({"increment":0}));
    assert!(child.try_wait().unwrap().is_none());
    fs::write(&gate, "release").unwrap();
    let mut rest = String::new();
    reader.read_to_string(&mut rest).unwrap();
    let mut increments = vec![0];
    let mut terminal = 0;
    for line in rest.lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(value) = record.get("increment") {
            increments.push(value.as_u64().unwrap());
        }
        if let Some(value) = record.get("total") {
            assert_eq!(value, 3);
            terminal += 1;
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(increments, [0, 1, 2]);
    assert_eq!(terminal, 1);
    let installed = root.path().join("installed");
    fs::copy(&executable, &installed).unwrap();
    fs::remove_file(root.path().join("flow.json")).unwrap();
    fs::remove_dir_all(root.path().join("build")).unwrap();
    let manifest = mf_tui::manifest::read_manifest(&installed).unwrap();
    assert!(manifest.description().unwrap().is_streaming());
    let output = Command::new(&installed)
        .args(["--count.count", "0"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        json!({"total":0})
    );
}

#[test]
fn combined_isolated_targets_and_finite_flows_keep_terminal_json_execution() {
    for streaming in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let executable = compile(root.path(), count_flow(streaming, None, None));
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
        if !streaming {
            let output = Command::new(&executable)
                .args(["--count.count", "2"])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
                json!({"total":2})
            );
        }
    }
}

#[test]
fn compiled_stream_contract_drift_fails_before_business_work() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    let executable = compile(root.path(), count_flow(true, None, Some(&marker)));
    let output = Command::new(&executable)
        .args(["--count.count", "1"])
        .env("MF_FIXTURE_STREAM_CONTRACT_CHANGE", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("metadata"));
}

#[test]
fn answer_and_title_share_one_result_while_only_answer_is_streamed() {
    use mfn_llm::test_support::{Reply, Server};
    let text = json!({"id":"answer","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{"content":"Live answer"},"finish_reason":null}]});
    let finish = json!({"id":"answer","object":"chat.completion.chunk","created":0,"model":"example-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
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
    for streaming in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let mut definition: serde_json::Value =
            serde_json::from_str(include_str!("../../../examples/chat-title-flow.json")).unwrap();
        for (alias, path) in [("core", "core"), ("code", "code"), ("llm", "llm")] {
            definition["dependencies"][alias]["path"] =
                json!(crates_dir().join("builtin-nodes").join(path));
        }
        if !streaming {
            definition.as_object_mut().unwrap().remove("execution");
            definition["outputs"]
                .as_array_mut()
                .unwrap()
                .retain(|output| output["name"] != "answer");
        }
        let executable = compile(root.path(), definition);
        let output = Command::new(&executable)
            .args([
                "--params.prompt",
                "Hello",
                "--params.base_url",
                &server.base_url,
            ])
            .env("OPENAI_API_KEY", "synthetic-key")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result = if streaming {
            let mut result = None;
            let mut deltas = 0;
            for line in output
                .stdout
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                let record: serde_json::Value = serde_json::from_slice(line).unwrap();
                if let Some(delta) = record.get("answer") {
                    assert_eq!(delta["text"], "Live answer");
                    deltas += 1;
                } else {
                    assert!(result.replace(record).is_none());
                }
            }
            assert_eq!(deltas, 1);
            result.unwrap()
        } else {
            serde_json::from_slice(&output.stdout).unwrap()
        };
        assert_eq!(result["text"], "Live answer");
        assert_eq!(result["title"], "Generated title");
        assert_eq!(result["usages"].as_array().unwrap().len(), 2);
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].body["stream"], true);
    for request in &requests[1..] {
        assert_ne!(request.body["stream"], true);
    }
}
