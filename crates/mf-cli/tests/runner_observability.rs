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
    for name in ["lib.rs", "context_fixture.rs"] {
        fs::copy(fixture.join(name), plugin.join("src").join(name)).unwrap();
    }
    let lib = plugin.join("src/lib.rs");
    let source = fs::read_to_string(&lib).unwrap();
    let minimal = source
        .replace("mod typed_fixture;\n", "")
        .replace("mod line_producer;\n", "")
        .replace("mod stream_fixture;\n", "");
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
    // Configuration-only rebuilds must retain the dependency versions warmed for this run.
    fs::copy(project.join("Cargo.lock"), flow_lock).unwrap();
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
                        {"from_node": "%loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
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

struct CollectorHandle {
    stop: mpsc::Sender<()>,
    exported: mpsc::Receiver<String>,
    worker: thread::JoinHandle<CapturedRequests>,
}

impl CollectorHandle {
    fn wait_for_exports(&self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut logs = false;
        let mut traces = false;
        while !logs || !traces {
            let path = self
                .exported
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("OTLP logs and traces did not arrive");
            match path.as_str() {
                "/v1/logs" => logs = true,
                "/v1/traces" => traces = true,
                other => panic!("unexpected OTLP endpoint {other}"),
            }
        }
    }

    fn finish(self) -> CapturedRequests {
        drop(self.stop);
        self.worker.join().unwrap()
    }
}

fn collector() -> (String, CollectorHandle) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let (stop, stopped) = mpsc::channel();
    let (exported, exports) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut handlers = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            assert!(Instant::now() < deadline, "OTLP collector was not stopped");
            match listener.accept() {
                Ok((stream, _)) => {
                    // An idle connection must not block a concurrent logs or traces request.
                    let exported = exported.clone();
                    handlers.push(thread::spawn(move || {
                        let request = read_request(stream);
                        if let Some((path, _)) = &request {
                            let _ = exported.send(path.clone());
                        }
                        request
                    }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("collector accept failed: {error}"),
            }
        }
        // A runner can exit after the HTTP response but before its handler returns.
        handlers
            .into_iter()
            .filter_map(|handler| handler.join().unwrap())
            .collect()
    });
    (
        endpoint,
        CollectorHandle {
            stop,
            exported: exports,
            worker,
        },
    )
}

fn check_otel(
    requests: &CapturedRequests,
    expected_node_count: usize,
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
            .filter(|event| event.event_name == "mf.workflow.started")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_name == "mf.workflow.finished")
            .count(),
        1
    );
    for event_name in ["mf.node.started", "mf.node.finished"] {
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_name == event_name)
                .count(),
            expected_node_count
        );
    }
    let mut events_by_sequence: Vec<_> = events.iter().collect();
    events_by_sequence.sort_by_key(|event| {
        event
            .attributes
            .iter()
            .find(|attribute| attribute.key == "mf.event.sequence")
            .and_then(|attribute| attribute.value.as_ref())
            .and_then(|value| value.value.as_ref())
            .and_then(|value| match value {
                any_value::Value::IntValue(sequence) => Some(*sequence),
                _ => None,
            })
            .unwrap()
    });
    assert_eq!(
        events_by_sequence.first().unwrap().event_name,
        "mf.workflow.started"
    );
    assert_eq!(
        events_by_sequence.last().unwrap().event_name,
        "mf.workflow.finished"
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
    for (index, event) in events_by_sequence.iter().enumerate() {
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
    let failed = events.iter().any(|event| event.attributes.iter().any(|attribute| {
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
    let dependency_lock = fs::read(&flow_lock).unwrap();
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

    let (endpoint, worker) = collector();
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
    let requests = worker.finish();
    check_otel(&requests, 3, 4, description.workflow_id.as_str());
    for (_, body) in requests.iter().filter(|(path, _)| path == "/v1/logs") {
        let export = ExportLogsServiceRequest::decode(body.as_slice()).unwrap();
        assert!(export.resource_logs.iter().flat_map(|resource| &resource.scope_logs)
            .flat_map(|scope| &scope.log_records).all(|record| {
                record.attributes.iter().any(|attribute| {
                    attribute.key == "mf.run.id" && matches!(attribute.value.as_ref().and_then(|value| value.value.as_ref()),
                        Some(any_value::Value::StringValue(id)) if id == "12345678-1234-4234-9234-123456789abc")
                })
            }));
    }

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

    assert_eq!(receiver.history_len(), 0);
    let run_id = mf_telemetry::identity::RunId::new();
    let mut history_receiver = LoopbackReceiver::bind(description.clone(), run_id).unwrap();
    let recorded = command(&runner)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", history_receiver.endpoint())
        .env("MF_RUN_ID", run_id.to_string())
        .env(mf_telemetry::SNAPSHOT_CAPTURE_ENV, "1")
        .output()
        .unwrap();
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert_eq!(recorded.stdout, plain.stdout);
    history_receiver.finish();
    let history = history_receiver.history_snapshot(None, 10);
    assert_eq!(history.status, "Complete");
    assert_eq!(history.history_len, 6);
    let history_root = &history.entries.last().unwrap().snapshot;
    assert!(
        history_root.node(&[], "a").unwrap().outputs["value"]
            .ptr_eq(&history_root.node(&[], "b").unwrap().inputs["input"])
    );
    assert_eq!(
        history_root.node(&[], "b").unwrap().outputs["result.alpha"],
        json!(14)
    );

    let (endpoint, worker) = collector();
    let logs_only = command(&runner)
        .env(
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            format!("{endpoint}/v1/logs"),
        )
        .output()
        .unwrap();
    assert!(logs_only.status.success());
    assert_eq!(logs_only.stdout, plain.stdout);
    let requests = worker.finish();
    assert_eq!(
        requests
            .iter()
            .flat_map(|(path, body)| {
                assert_eq!(path, "/v1/logs");
                ExportLogsServiceRequest::decode(body.as_slice())
                    .unwrap()
                    .resource_logs
            })
            .flat_map(|resource| resource.scope_logs)
            .flat_map(|scope| scope.log_records)
            .count(),
        8
    );

    let (endpoint, worker) = collector();
    let traces_only = command(&runner)
        .env(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            format!("{endpoint}/v1/traces"),
        )
        .output()
        .unwrap();
    assert!(traces_only.status.success());
    assert_eq!(traces_only.stdout, plain.stdout);
    let requests = worker.finish();
    assert_eq!(
        requests
            .iter()
            .flat_map(|(path, body)| {
                assert_eq!(path, "/v1/traces");
                ExportTraceServiceRequest::decode(body.as_slice())
                    .unwrap()
                    .resource_spans
            })
            .flat_map(|resource| resource.scope_spans)
            .flat_map(|scope| scope.spans)
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
    assert_eq!(fs::read(&flow_lock).unwrap(), dependency_lock);
    assert!(
        command(&failing_runner)
            .arg("--validate")
            .output()
            .unwrap()
            .status
            .success()
    );
    let failing_description = describe_executable(&failing_runner).unwrap();
    let (endpoint, worker) = collector();
    let failed = command(&failing_runner)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("execution sentinel"));
    let requests = worker.finish();
    check_otel(&requests, 3, 4, failing_description.workflow_id.as_str());

    fs::remove_dir_all(&project).unwrap();
    fs::remove_dir_all(&plugin).unwrap();
    fs::remove_file(&flow_lock).unwrap();
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
    let trace = fs::read_to_string(&trace).unwrap();
    let mut trace_entries: Vec<_> = trace.lines().collect();
    trace_entries.sort_unstable();
    assert_eq!(trace_entries, ["b", "c"]);
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
    impl mf_compiler::TaskNode for EmptyNode {
        fn execute(
            &self,
            _: mf_compiler::Inputs,
            _: &mut mf_compiler::ExecutionContext,
        ) -> Result<mf_compiler::NodeResult, mf_compiler::NodeExecutionError> {
            Ok(mf_compiler::Outputs::new().into())
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
                mf_compiler::PreparedNode::new(EmptyNode, mf_compiler::NodePorts::default()),
            )
        })
        .collect();
    let order = names
        .iter()
        .cloned()
        .map(mf_compiler::DefinitionId::from)
        .collect();
    let flow = mf_compiler::build_flow(nodes, vec![], order, vec![]).unwrap();
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

#[test]
fn streaming_runner_exports_message_lifecycles_without_changing_results() {
    use mf_telemetry::{
        stream::{StreamEvent, StreamOutcome, StreamPayload, StreamRecord},
        wire::WireRecord,
    };
    use std::process::Stdio;
    if !loopback_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut value: Value =
        serde_json::from_str(include_str!("../../../examples/stream-batch.json")).unwrap();
    value["dependencies"]["core"]["path"] = json!(crates_dir().join("builtin-nodes/core"));
    value["dependencies"]["code"]["path"] = json!(crates_dir().join("builtin-nodes/code"));
    value["nodes"][0]["config"]["max_wait_ms"] = json!(3_600_000);
    let definition: WorkflowDefinition = serde_json::from_value(value).unwrap();
    let runner = build(
        &root.path().join("build"),
        &root.path().join("flow.lock"),
        &definition,
    );
    let execute = |endpoint: Option<&str>, collector: Option<&CollectorHandle>| {
        let mut command = command(&runner);
        if let Some(endpoint) = endpoint {
            command.env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(b"1\n2\n3\n").unwrap();
        if let Some(collector) = collector {
            // Force later events into new export requests without relying on sleeps.
            collector.wait_for_exports();
        }
        input.write_all(b"4\n5\n").unwrap();
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    let plain = execute(None, None);
    let (endpoint, collector) = collector();
    let observed = execute(Some(&endpoint), Some(&collector));
    assert_eq!(observed.stdout, plain.stdout);
    let requests = collector.finish();
    assert!(
        requests
            .iter()
            .filter(|(path, _)| path == "/v1/logs")
            .count()
            >= 2
    );
    let mut records = Vec::new();
    let mut spans = Vec::new();
    for (path, body) in requests {
        if path == "/v1/logs" {
            let export = ExportLogsServiceRequest::decode(body.as_slice()).unwrap();
            for scope in export
                .resource_logs
                .into_iter()
                .flat_map(|resource| resource.scope_logs)
            {
                let name = scope.scope.unwrap().name;
                for record in scope.log_records {
                    assert_eq!(record.trace_id.len(), 16);
                    assert_eq!(record.span_id.len(), 8);
                    let wire = WireRecord {
                        scope: name.clone(),
                        event_name: record.event_name,
                        time_unix_nano: record.time_unix_nano,
                        trace_context: None,
                        attributes: record
                            .attributes
                            .into_iter()
                            .map(|attribute| {
                                (attribute.key, stream_proto_value(&attribute.value.unwrap()))
                            })
                            .collect(),
                        body: stream_proto_value(&record.body.unwrap()),
                    };
                    records.push(StreamRecord::decode(&wire).unwrap());
                }
            }
        } else if path == "/v1/traces" {
            let export = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
            spans.extend(
                export
                    .resource_spans
                    .into_iter()
                    .flat_map(|resource| resource.scope_spans)
                    .flat_map(|scope| scope.spans),
            );
        } else {
            panic!("unexpected exporter endpoint {path}");
        }
    }
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record.sequence.get(), index as i64 + 1);
    }
    assert!(records.iter().any(|record| matches!(
        record.payload,
        StreamPayload::Control(StreamEvent::Flushed { .. })
    )));
    assert!(
        matches!(&records.last().unwrap().payload, StreamPayload::Control(StreamEvent::Finished { outcome: StreamOutcome::Succeeded, counts, .. }) if counts.startup_frames == 1 && counts.emitted_messages == 7 && counts.delivered_outputs == 2),
        "unexpected terminal record: {:?}",
        records.last()
    );
    assert_eq!(spans.len(), 15);
    assert!(
        spans
            .iter()
            .filter(|span| span.name == "mf.node")
            .all(|span| span
                .attributes
                .iter()
                .any(|attribute| attribute.key == "mf.stream.invocation"))
    );
    let run_id = mf_telemetry::identity::RunId::new();
    let described = mf_compiler::describe_compiled(&plan_definition(&definition).unwrap()).unwrap();
    let mut receiver = LoopbackReceiver::bind(described, run_id).unwrap();
    let mut child = command(&runner)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", receiver.endpoint())
        .env("MF_RUN_ID", run_id.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"1\n2\n3\n4\n5\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, plain.stdout);
    let snapshot = receiver.finish();
    assert_eq!(
        snapshot.lifecycle.completeness,
        Completeness::Complete,
        "{snapshot:?}"
    );
    assert!(
        snapshot
            .nodes
            .iter()
            .all(|node| node.status == NodeStatus::Succeeded)
    );
    assert_eq!(
        snapshot.stream.unwrap().counts.unwrap().delivered_outputs,
        2
    );
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    assert_eq!(
        execute(Some(&format!("http://{closed}")), None).stdout,
        plain.stdout
    );
}

fn stream_proto_value(value: &opentelemetry_proto::tonic::common::v1::AnyValue) -> Value {
    match value.value.as_ref().unwrap() {
        any_value::Value::StringValue(value) => json!(value),
        any_value::Value::IntValue(value) => json!(value),
        any_value::Value::BoolValue(value) => json!(value),
        any_value::Value::ArrayValue(values) => {
            values.values.iter().map(stream_proto_value).collect()
        }
        any_value::Value::KvlistValue(values) => Value::Object(
            values
                .values
                .iter()
                .map(|value| {
                    (
                        value.key.clone(),
                        stream_proto_value(value.value.as_ref().unwrap()),
                    )
                })
                .collect(),
        ),
        _ => panic!("unexpected stream metadata value"),
    }
}
