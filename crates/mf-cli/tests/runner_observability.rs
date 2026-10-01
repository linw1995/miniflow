#![cfg(unix)]

use mf_compiler::{
    CompileRequest, SupportPackages, WorkflowDefinition, compile_project, plan_definition,
    resolve_project, write_dependency_project,
};
use mf_tui::{
    description::describe_executable,
    receiver::LoopbackReceiver,
    state::{Completeness, NodeStatus},
};
use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::any_value,
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn plugin(root: &Path) -> PathBuf {
    let plugin = root.join("plugin");
    fs::create_dir_all(plugin.join("src")).unwrap();
    let fixture = crates_dir().join("mf-compiler/tests/fixtures/multi-nodes/src");
    for name in ["lib.rs", "context_fixture.rs", "subgraph_fixture.rs"] {
        fs::copy(fixture.join(name), plugin.join("src").join(name)).unwrap();
    }
    let lib = plugin.join("src/lib.rs");
    let source = fs::read_to_string(&lib).unwrap();
    let minimal = source.replace("mod typed_fixture;\n", "");
    assert_ne!(minimal, source);
    fs::write(lib, minimal).unwrap();
    let runtime = serde_json::to_string(&crates_dir().join("mf-runtime")).unwrap();
    fs::write(
        plugin.join("Cargo.toml"),
        format!(
            r#"
[package]
name = "fixture-multi-nodes"
version = "0.1.0"
edition = "2024"

[workspace]

[features]
default = []
fail-execution = []
duplicate-kind = []
double = []

[dependencies]
mf-runtime = {{ path = {runtime}, version = "0.1.0" }}
inventory = "0.3.24"
serde = {{ version = "1.0.229", features = ["derive"] }}
serde_json = "1.0.151"
"#
        ),
    )
    .unwrap();
    plugin
}

fn definition(plugin: &Path, trace: &Path) -> WorkflowDefinition {
    serde_json::from_value(json!({
        "version":"2026-09-26",
        "dependencies":{"fixture":{"package":"fixture-multi-nodes","path":plugin}},
        "nodes":[
            {"id":"a","kind":"fixture.source","config":{
                "print":true,"credential":"configuration-sentinel"
            }},
            {"id":"b","kind":"fixture.context","config":{
                "ports":["result.alpha","unused"],"required":"result.alpha",
                "inputs":["input"],"outputs":{"result.alpha":14},"skipped":["unused"],
                "trace":trace,"name":"b"
            }},
            {"id":"c","kind":"fixture.context","config":{
                "ports":["other.port"],"required":"other.port",
                "outputs":{"other.port":true},
                "trace":trace,"name":"c"
            }}
        ],
        "edges":[{"from_node":"a","from_output":"value","to_node":"b","to_input":"input"}],
        "control_edges":[{"from_node":"a","from_output":"value","to_node":"c"}],
        "outputs":[{"name":"answer","node":"b","port":"result.alpha"}]
    }))
    .unwrap()
}

fn build(project: &Path, flow_lock: &Path, definition: &WorkflowDefinition) -> PathBuf {
    let plan = plan_definition(definition).unwrap();
    write_dependency_project(
        project,
        &plan,
        &SupportPackages::Local {
            crates_dir: crates_dir(),
        },
    )
    .unwrap();
    resolve_project(project, flow_lock, false).unwrap();
    let target = std::env::var_os("MF_TEST_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| project.join("target"));
    let output = mf_compiler::cargo_command(project)
        .args(["build", "--offline", "--release", "--locked"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    target.join(format!(
        "release/mf-generated-workflow{}",
        std::env::consts::EXE_SUFFIX
    ))
}

fn command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    for name in [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
        "MF_RUN_ID",
    ] {
        command.env_remove(name);
    }
    command
}

fn loopback_available() -> bool {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("could not probe loopback availability: {error}"),
    }
}

fn last_json(stdout: &[u8]) -> Value {
    let line = stdout
        .split(|byte| *byte == b'\n')
        .rfind(|line| !line.is_empty())
        .unwrap();
    serde_json::from_slice(line).unwrap()
}

#[test]
fn generated_loop_runner_exports_complete_per_pass_observations() {
    if !loopback_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let definition = root.path().join("loop.json");
    let executable = root.path().join("loop-runner");
    let build_dir = root.path().join("build");
    fs::write(&definition, json!({
        "version": "2026-09-29",
        "dependencies": {
            "core": {"package": "mfn-core", "path": crates_dir().join("builtin-nodes/core")},
            "code": {"package": "mfn-code", "path": crates_dir().join("builtin-nodes/code")}
        },
        "nodes": [
            {"id": "seed", "kind": "builtin.constant", "config": {"value": 0}},
            {"id": "repeat", "kind": "workflow.loop", "loop": {
                "max_iterations": 5,
                "variables": [{"name": "count", "type": "int"}],
                "until": {"variable": "count", "operator": "gte", "value": 3},
                "body": {
                    "nodes": [
                        {"id": "increment", "kind": "builtin.code", "config": {
                            "language": "cel", "inputs": {"count": "int"},
                            "code": {"next": "count + 1"}
                        }},
                        {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
                    ],
                    "edges": [
                        {"from_node": "$loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
                        {"from_node": "increment", "from_output": "next", "to_node": "assign", "to_input": "value"}
                    ]
                }
            }}
        ],
        "edges": [{"from_node": "seed", "from_output": "value", "to_node": "repeat", "to_input": "count"}],
        "outputs": [{"name": "count", "node": "repeat", "port": "count"}]
    }).to_string()).unwrap();
    compile_project(&CompileRequest {
        definition: &definition,
        output: &executable,
        locked: false,
        build_dir: Some(&build_dir),
        support: &SupportPackages::Local {
            crates_dir: crates_dir(),
        },
    })
    .unwrap();
    let description = describe_executable(&executable).unwrap();
    assert_eq!(description.loop_bodies.len(), 1);
    let description_json = serde_json::to_string(&description).unwrap();
    assert!(!description_json.contains("count + 1"));
    assert!(!description_json.contains("max_iterations"));
    let run_id = mf_telemetry::identity::RunId::new();
    let mut receiver = LoopbackReceiver::bind(description, run_id).unwrap();
    let result = command(&executable)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", receiver.endpoint())
        .env("MF_RUN_ID", run_id.to_string())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(last_json(&result.stdout), json!({"count": 3}));
    let snapshot = receiver.finish();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
    assert_eq!(snapshot.total_loop_passes, 3);
    assert_eq!(snapshot.loop_passes.len(), 3);
    assert_eq!(snapshot.loop_overviews[0].completed_passes, 3);
}

fn read_request(mut stream: TcpStream) -> Option<(String, Vec<u8>)> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut content = Vec::new();
    let mut chunk = [0u8; 8192];
    let boundary = loop {
        let size = match stream.read(&mut chunk) {
            Ok(0) if content.is_empty() => return None,
            Err(error)
                if content.is_empty()
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
            {
                return None;
            }
            result => result.unwrap(),
        };
        assert!(size != 0, "request headers ended early");
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        content.extend_from_slice(&chunk[..size]);
        assert!(
            content.len() <= 8 * 1024 * 1024,
            "request exceeded configured limit"
        );
        if let Some(end) = content.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = std::str::from_utf8(&content[..boundary]).unwrap();
    let path = headers.split_whitespace().nth(1).unwrap().to_owned();
    let len = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        })
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .expect("OTLP request must have Content-Length");
    assert!(len <= 8 * 1024 * 1024);
    while content.len() < boundary + len {
        let size = stream.read(&mut chunk).unwrap();
        assert!(size != 0, "OTLP body ended early");
        content.extend_from_slice(&chunk[..size]);
    }
    let body = content[boundary..boundary + len].to_vec();
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-protobuf\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    Some((path, body))
}

type CapturedRequests = Vec<(String, Vec<u8>)>;
type CollectorHandle = thread::JoinHandle<CapturedRequests>;

fn collector(expected: usize) -> (String, CollectorHandle) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        let (sender, receiver) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_secs(8);
        while requests.len() < expected && Instant::now() < deadline {
            requests.extend(receiver.try_iter());
            match listener.accept() {
                Ok((stream, _)) => {
                    // An idle connection must not block a concurrent logs or traces request.
                    let sender = sender.clone();
                    thread::spawn(move || {
                        if let Some(request) = read_request(stream) {
                            let _ = sender.send(request);
                        }
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("collector accept failed: {error}"),
            }
        }
        requests.extend(receiver.try_iter());
        assert_eq!(requests.len(), expected, "OTLP requests did not arrive");
        requests
    });
    (endpoint, worker)
}

fn check_otel(
    requests: &CapturedRequests,
    expected_events: &[&str],
    expected_spans: usize,
    workflow_id: &str,
) {
    let mut events = Vec::new();
    let mut spans = Vec::new();
    for (path, body) in requests {
        match path.as_str() {
            "/v1/logs" => {
                let export = ExportLogsServiceRequest::decode(body.as_slice()).unwrap();
                for resource in export.resource_logs {
                    assert_service_name(resource.resource.as_ref().unwrap().attributes.as_slice());
                    for scope in resource.scope_logs {
                        assert_eq!(scope.scope.unwrap().name, "mf.workflow");
                        events.extend(scope.log_records);
                    }
                }
            }
            "/v1/traces" => {
                let export = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
                for resource in export.resource_spans {
                    assert_service_name(resource.resource.as_ref().unwrap().attributes.as_slice());
                    for scope in resource.scope_spans {
                        spans.extend(scope.spans);
                    }
                }
            }
            other => panic!("unexpected OTLP endpoint {other}"),
        }
    }
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_name.as_str())
            .collect::<Vec<_>>(),
        expected_events
    );
    assert_eq!(spans.len(), expected_spans);
    assert!(!requests.iter().any(|(_, body)| {
        body.windows(b"configuration-sentinel".len())
            .any(|window| window == b"configuration-sentinel")
    }));
    let root = spans
        .iter()
        .find(|span| span.name == "mf.workflow")
        .unwrap();
    assert_eq!(root.trace_id.len(), 16);
    assert!(
        events
            .iter()
            .all(|event| event.trace_id == root.trace_id && event.span_id.len() == 8)
    );
    for (index, event) in events.iter().enumerate() {
        assert!(event.attributes.iter().any(|attribute| {
            attribute.key == "mf.workflow.id"
                && matches!(attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                Some(any_value::Value::StringValue(id)) if id == workflow_id)
        }));
        let value = event
            .attributes
            .iter()
            .find(|attribute| attribute.key == "mf.event.sequence")
            .unwrap()
            .value
            .as_ref()
            .unwrap();
        assert!(
            matches!(value.value, Some(any_value::Value::IntValue(sequence)) if sequence == (index + 1) as i64)
        );
    }
    let failed = expected_events.contains(&"mf.node.finished")
        && events.iter().any(|event| event.attributes.iter().any(|attribute| {
            attribute.key == "mf.outcome" && matches!(attribute.value.as_ref().and_then(|value| value.value.as_ref()), Some(any_value::Value::StringValue(outcome)) if outcome == "failed")
        }));
    if failed {
        assert_eq!(root.status.as_ref().unwrap().code, 2);
        assert!(spans.iter().any(|span| span.name == "mf.node"
            && span.status.as_ref().is_some_and(|status| status.code == 2)));
    }
}

fn assert_service_name(attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue]) {
    assert!(attributes.iter().any(|attribute| {
        attribute.key == "service.name"
            && matches!(attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                Some(any_value::Value::StringValue(name)) if name == "mf-generated-workflow")
    }));
}

#[test]
fn generated_runner_describes_embedded_graph_and_exports_correlated_otel() {
    let root = tempfile::tempdir().unwrap();
    let plugin = plugin(root.path());
    let project = root.path().join("build");
    let flow_lock = root.path().join("flow.lock");
    let trace = root.path().join("execution.trace");
    let definition = definition(&plugin, &trace);
    let runner = build(&project, &flow_lock, &definition);
    let validation = command(&runner).arg("--validate").output().unwrap();
    assert!(
        validation.status.success(),
        "{}",
        String::from_utf8_lossy(&validation.stderr)
    );
    assert!(
        validation
            .stdout
            .windows(b"factory diagnostic".len())
            .any(|w| w == b"factory diagnostic")
    );
    let raw = command(&runner).arg("--describe").output().unwrap();
    assert!(
        raw.status.success(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
    assert!(raw.stdout.ends_with(b"\n"));
    assert_eq!(raw.stdout.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert!(
        !raw.stdout
            .windows(b"diagnostic".len())
            .any(|w| w == b"diagnostic")
    );
    assert!(raw.stderr.is_empty());
    assert!(
        !trace.exists(),
        "description constructed or executed a plugin"
    );
    let description = describe_executable(&runner).unwrap();
    assert_eq!(
        raw.stdout[..raw.stdout.len() - 1],
        description.to_json().unwrap()
    );
    assert_eq!(description.execution_order, ["a", "b", "c"]);
    assert_eq!(description.data_edges.len(), 1);
    assert_eq!(description.control_edges.len(), 1);
    assert_eq!(description.nodes[1].id, "b");
    assert_eq!(description.nodes[1].kind, "fixture.context");
    assert_eq!(description.data_edges[0].from_output, "value");
    assert_eq!(description.data_edges[0].to_input, "input");
    assert_eq!(description.control_edges[0].from_output, "value");
    assert!(
        serde_json::from_slice::<Value>(&raw.stdout).unwrap()["nodes"][0]
            .get("outputs")
            .is_none()
    );
    assert!(!String::from_utf8_lossy(&raw.stdout).contains("configuration-sentinel"));

    if !loopback_available() {
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    assert!(
        command(&runner)
            .arg("--validate")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        command(&runner)
            .arg("--describe")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );

    let plain = command(&runner).output().unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    assert_eq!(last_json(&plain.stdout), json!({"answer":14}));

    let (endpoint, worker) = collector(2);
    let observed = command(&runner)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")
        .env("MF_RUN_ID", "12345678-1234-4234-9234-123456789abc")
        .output()
        .unwrap();
    assert!(
        observed.status.success(),
        "{}",
        String::from_utf8_lossy(&observed.stderr)
    );
    assert_eq!(observed.stdout, plain.stdout);
    let requests = worker.join().unwrap();
    check_otel(
        &requests,
        &[
            "mf.workflow.started",
            "mf.node.started",
            "mf.node.finished",
            "mf.node.started",
            "mf.node.finished",
            "mf.node.started",
            "mf.node.finished",
            "mf.workflow.finished",
        ],
        4,
        description.workflow_id.as_str(),
    );
    let logs = requests
        .iter()
        .find(|(path, _)| path == "/v1/logs")
        .unwrap();
    let export = ExportLogsServiceRequest::decode(logs.1.as_slice()).unwrap();
    assert!(export.resource_logs.iter().flat_map(|resource| &resource.scope_logs)
        .flat_map(|scope| &scope.log_records).all(|record| {
            record.attributes.iter().any(|attribute| {
                attribute.key == "mf.run.id" && matches!(attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                    Some(any_value::Value::StringValue(id)) if id == "12345678-1234-4234-9234-123456789abc")
            })
        }));

    let run_id =
        mf_telemetry::identity::RunId::try_from("12345678-1234-4234-9234-123456789abd".to_owned())
            .unwrap();
    let mut receiver = LoopbackReceiver::bind(description.clone(), run_id).unwrap();
    let received = command(&runner)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", receiver.endpoint())
        .env("MF_RUN_ID", run_id.to_string())
        .output()
        .unwrap();
    assert!(received.status.success());
    assert_eq!(received.stdout, plain.stdout);
    let snapshot = receiver.finish();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
    assert_eq!(snapshot.lifecycle.known_missing_count, 0);
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.status == NodeStatus::Succeeded)
    );
    assert_eq!(snapshot.traces.observed_spans, 4);

    let (endpoint, worker) = collector(1);
    let logs_only = command(&runner)
        .env(
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            format!("{endpoint}/v1/logs"),
        )
        .output()
        .unwrap();
    assert!(logs_only.status.success());
    assert_eq!(logs_only.stdout, plain.stdout);
    let requests = worker.join().unwrap();
    assert_eq!(requests[0].0, "/v1/logs");
    let logs = ExportLogsServiceRequest::decode(requests[0].1.as_slice()).unwrap();
    assert_eq!(
        logs.resource_logs
            .iter()
            .flat_map(|resource| &resource.scope_logs)
            .flat_map(|scope| &scope.log_records)
            .count(),
        8
    );

    let (endpoint, worker) = collector(1);
    let traces_only = command(&runner)
        .env(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            format!("{endpoint}/v1/traces"),
        )
        .output()
        .unwrap();
    assert!(traces_only.status.success());
    assert_eq!(traces_only.stdout, plain.stdout);
    let requests = worker.join().unwrap();
    assert_eq!(requests[0].0, "/v1/traces");
    let traces = ExportTraceServiceRequest::decode(requests[0].1.as_slice()).unwrap();
    assert_eq!(
        traces
            .resource_spans
            .iter()
            .flat_map(|resource| &resource.scope_spans)
            .flat_map(|scope| &scope.spans)
            .count(),
        4
    );

    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let unavailable = command(&runner)
        .env("MF_RUN_ID", "invalid-run-id")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", format!("http://{closed}"))
        .output()
        .unwrap();
    assert!(unavailable.status.success());
    assert_eq!(unavailable.stdout, plain.stdout);
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("invalid MF_RUN_ID"));

    let portable = root.path().join("portable-runner");
    fs::copy(&runner, &portable).unwrap();
    fs::set_permissions(&portable, fs::Permissions::from_mode(0o700)).unwrap();

    let mut failing = definition;
    failing.nodes[1].config["fail"] = json!(true);
    let failing_runner = build(&project, &flow_lock, &failing);
    assert!(
        command(&failing_runner)
            .arg("--validate")
            .output()
            .unwrap()
            .status
            .success()
    );
    let failing_description = describe_executable(&failing_runner).unwrap();
    let (endpoint, worker) = collector(2);
    let failed = command(&failing_runner)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("execution sentinel"));
    let requests = worker.join().unwrap();
    check_otel(
        &requests,
        &[
            "mf.workflow.started",
            "mf.node.started",
            "mf.node.finished",
            "mf.node.started",
            "mf.node.finished",
            "mf.workflow.finished",
        ],
        3,
        failing_description.workflow_id.as_str(),
    );

    fs::remove_dir_all(&project).unwrap();
    fs::remove_dir_all(&plugin).unwrap();
    assert!(!flow_lock.exists());
    fs::remove_file(&trace).unwrap();
    let portable_description = command(&portable)
        .arg("--describe")
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(portable_description.status.success());
    assert_eq!(portable_description.stdout, raw.stdout);
    let portable_output = command(&portable).env("PATH", "").output().unwrap();
    assert!(portable_output.status.success());
    assert_eq!(last_json(&portable_output.stdout), json!({"answer":14}));
    assert_eq!(fs::read_to_string(&trace).unwrap(), "b\nc\n");
}

#[test]
fn blocked_otel_export_does_not_block_flow_execution() {
    if !loopback_available() {
        return;
    }
    if std::env::var_os("MF_TEST_EXPORT_STRESS").is_none() {
        let (endpoint, worker) = stalled_collector();
        let output = command(&std::env::current_exe().unwrap())
            .args([
                "--exact",
                "blocked_otel_export_does_not_block_flow_execution",
            ])
            .env("MF_TEST_EXPORT_STRESS", "1")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            worker.join().unwrap(),
            "exporter never contacted the collector"
        );
        return;
    }

    struct EmptyNode;
    impl mf_compiler::Node for EmptyNode {
        fn execute(
            &self,
            _: mf_compiler::Inputs,
        ) -> Result<mf_compiler::Outputs, mf_compiler::NodeExecutionError> {
            Ok(mf_compiler::Outputs::new())
        }
    }

    let providers = mf_telemetry::otlp::TelemetryProviders::from_env()
        .unwrap()
        .unwrap();
    let count = mf_telemetry::otlp::MAX_QUEUE_SIZE + 256;
    let names: Vec<String> = (0..count).map(|i| format!("node_{i:04}")).collect();
    let nodes = names
        .iter()
        .map(|id| {
            mf_compiler::FlowNode::new(
                id.clone(),
                Box::new(EmptyNode),
                mf_compiler::NodePorts::default(),
            )
        })
        .collect();
    let order = names
        .iter()
        .cloned()
        .map(mf_compiler::DefinitionId::from)
        .collect();
    let flow = mf_compiler::Flow::new(nodes, vec![], order, vec![]).unwrap();
    let identity =
        mf_telemetry::identity::WorkflowId::from_definition(&json!({"node_count":count}), &names)
            .unwrap();
    let identities = names
        .into_iter()
        .map(|id| mf_telemetry::event::NodeIdentity {
            id,
            kind: "fixture.empty".into(),
            path: Vec::new(),
        })
        .collect();
    let observation = providers
        .observer()
        .start(identity, mf_telemetry::identity::RunId::new(), identities)
        .unwrap();
    let started = Instant::now();
    let observed = flow.execute_with_observation(Some(observation)).unwrap();
    assert_eq!(observed, flow.execute().unwrap());
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "an unavailable exporter blocked node execution"
    );
    let _ = providers.shutdown();
}

fn stalled_collector() -> (String, thread::JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    thread::sleep(Duration::from_secs(3));
                    drop(stream);
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("collector accept failed: {error}"),
            }
        }
        false
    });
    (endpoint, worker)
}
