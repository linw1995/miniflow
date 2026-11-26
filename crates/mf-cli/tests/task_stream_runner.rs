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
fn installed_finite_runner_calls_a_native_host_without_chat_or_flow_sources() {
    use mf_runtime::callback::{CallbackReply, HostCallback, HostCallbacks};
    use mf_tui::runner::{PreparedRunner, RunnerInvocation};
    use std::io::Write;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let root = tempfile::tempdir().unwrap();
    let executable = compile(
        root.path(),
        json!({
            "version":"2026-11-13", "dependencies":{"core":{"package":"mfn-core","path":crates_dir().join("builtin-nodes/core")}},
            "nodes":[{"id":"lookup","kind":"workflow.call","config":{"flow":"callback:lookup","body":{
                "nodes":[{"id":"query","kind":"builtin.parameters","config":{"parameters":{"key":{"type":"string","default":"local"}}}}],
                "outputs":[{"name":"value","node":"query","port":"key"}]
            }}}],"outputs":[{"name":"type","node":"lookup","port":"value"}]
        }),
    );
    fs::remove_file(root.path().join("flow.json")).unwrap();
    fs::remove_dir_all(root.path().join("build")).unwrap();
    let normal = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(normal.status.success(), "{normal:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&normal.stdout).unwrap(),
        json!({"type":"local"})
    );
    let runner = PreparedRunner::load(&executable, 1024).unwrap();
    assert_eq!(runner.manifest().descriptor.callbacks["lookup"], ["lookup"]);
    let committed = Arc::new(AtomicUsize::new(0));
    let capture = committed.clone();
    let handler = HostCallback::new(move |args, _| {
        assert_eq!(args.0["query.key"].as_str(), Some("local"));
        assert!(args.0["query.key"].is_secret());
        let capture = capture.clone();
        Ok(
            CallbackReply::from(mf_runtime::Outputs::from([("value".into(), "call".into())]))
                .on_delivery(move |ok| {
                    assert!(ok);
                    capture.fetch_add(1, Ordering::SeqCst);
                }),
        )
    });
    let callbacks: HostCallbacks = [("lookup".into(), handler)].into();
    let run = |callbacks| {
        RunnerInvocation {
            runner: &runner,
            arguments: &Default::default(),
            verbose: 0,
            snapshots: false,
        }
        .run_with_callbacks(
            callbacks,
            Default::default(),
            |_| {},
            |records| {
                assert!(records.next().is_none());
                Ok::<_, std::convert::Infallible>(())
            },
            || false,
        )
    };
    let finished = run(callbacks.clone()).unwrap().outcome;
    assert!(finished.status.success(), "{:?}", finished.capture_error);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&finished.stdout).unwrap(),
        json!({"type":"call"})
    );
    assert_eq!(committed.load(Ordering::SeqCst), 1);
    assert!(run([("other".into(), callbacks["lookup"].clone())].into()).is_err());
    // Even a live keyboard or a pipe which never reaches EOF is not a workflow input.
    let (open_input, _writer) = std::io::pipe().unwrap();
    let pty = nix::pty::openpty(None, None).unwrap();
    for input in [
        Stdio::from(open_input),
        Stdio::from(pty.slave),
        Stdio::null(),
    ] {
        let mut child = Command::new(&executable)
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("bare runner waited for user input");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, normal.stdout);
    }
    let null_output = Command::new(&executable)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(null_output.success());
    use mf_runtime::callback_rpc::LAUNCH_ENV;
    let path = root.path().join("rpc");
    let grant = |bindings: &[&str]| {
        json!({"parent":std::process::id(),"socket":path,"bindings":bindings}).to_string()
    };
    for (args, config, message) in [
        (vec![], grant(&["other"]), "unknown or duplicate"),
        (vec![], grant(&["lookup", "lookup"]), "unknown or duplicate"),
        (
            vec!["--node=lookup"],
            grant(&["lookup"]),
            "unknown or duplicate",
        ),
        (vec![], grant(&["lookup"]), "I/O failed"),
        (vec![], String::from("{"), "encoding failed"),
    ] {
        let output = Command::new(&executable)
            .args(args)
            .env(LAUNCH_ENV, config)
            .output()
            .unwrap();
        assert!(
            !output.status.success() && output.stdout.is_empty(),
            "{output:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{output:?}"
        );
    }
    let foreign = Command::new(&executable)
        .env(
            LAUNCH_ENV,
            json!({"parent":0,"socket":path,"bindings":["lookup"]}).to_string(),
        )
        .output()
        .unwrap();
    assert!(foreign.status.success() && foreign.stdout == normal.stdout);
    let server = std::os::unix::net::UnixListener::bind(&path).unwrap();
    for (response, diagnostic) in [
        ("", "host closed without a result"),
        ("{", "incomplete RPC"),
        (
            "{\"id\":2,\"value\":{\"values\":{}}}\n",
            "unexpected callback result ID",
        ),
        (
            "{\"id\":1,\"value\":{\"values\":{}}}\n",
            "missing callback output",
        ),
    ] {
        let child = Command::new(&executable)
            .env(LAUNCH_ENV, grant(&["lookup"]))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (mut peer, _) = server.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&peer).read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            json!({"id":1,"binding":"lookup","args":{"values":{"query.key":"local"},"private":[["query.key"]]}})
        );
        peer.write_all(response.as_bytes()).unwrap();
        drop(peer);
        let result = child.wait_with_output().unwrap();
        assert!(
            !result.status.success() && result.stdout.is_empty(),
            "{result:?}"
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(diagnostic),
            "{result:?}"
        );
    }
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
                "--params.messages",
                r#"[{"role":"user","content":"Hello"}]"#,
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
        assert_eq!(result["append"]["messages"][0]["content"], "Live answer");
        assert_eq!(result["append"]["title"], "Generated title");
        assert_eq!(result["append"]["usages"].as_array().unwrap().len(), 2);
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].body["stream"], true);
    for request in &requests[1..] {
        assert_ne!(request.body["stream"], true);
    }
}
