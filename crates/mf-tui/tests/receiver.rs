use mf_telemetry::{
    description::{WorkflowDescription, WorkflowDescriptionVersion},
    identity::{RunId, WorkflowId},
};
use mf_tui::{
    receiver::{LoopbackReceiver, MAX_REQUEST_BYTES},
    state::Completeness,
};
use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::{AnyValue, InstrumentationScope, KeyValue, KeyValueList, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    trace::v1::{ResourceSpans, ScopeSpans, Span},
};
use prost::Message;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};

fn loopback_available() -> bool {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("could not probe loopback: {error}"),
    }
}

fn graph() -> WorkflowDescription {
    WorkflowDescription {
        version: WorkflowDescriptionVersion::CURRENT,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
        nodes: vec![],
        data_edges: vec![],
        control_edges: vec![],
        execution_order: vec![],
    }
}

fn run_id() -> RunId {
    RunId::try_from("12345678-1234-4234-9234-123456789abc".to_owned()).unwrap()
}

fn string(value: impl Into<String>) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.into())),
    }
}

fn integer(value: i64) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::IntValue(value)),
    }
}

fn attribute(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(value),
        ..Default::default()
    }
}

fn body(entries: &[(&str, i64)]) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: entries
                .iter()
                .map(|(key, value)| attribute(key, integer(*value)))
                .collect(),
        })),
    }
}

fn record(sequence: i64, final_record: bool, run: &str, version: i64) -> LogRecord {
    let mut attributes = vec![
        attribute("mf.schema.version", integer(version)),
        attribute("mf.workflow.id", string(graph().workflow_id.to_string())),
        attribute("mf.run.id", string(run)),
        attribute("mf.event.sequence", integer(sequence)),
    ];
    if final_record {
        attributes.push(attribute("mf.outcome", string("succeeded")));
    }
    LogRecord {
        event_name: if final_record {
            "mf.workflow.finished"
        } else {
            "mf.workflow.started"
        }
        .into(),
        attributes,
        body: Some(if final_record {
            body(&[
                ("final_sequence", sequence),
                ("elapsed_ns", 0),
                ("visited_node_count", 0),
            ])
        } else {
            body(&[("node_count", 0), ("elapsed_ns", 0)])
        }),
        ..Default::default()
    }
}

fn logs(records: Vec<LogRecord>) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "mf.workflow".into(),
                    ..Default::default()
                }),
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

fn traces(run: &str) -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![1; 16],
                    span_id: vec![2; 8],
                    name: "mf.workflow".into(),
                    attributes: vec![
                        attribute("mf.workflow.id", string(graph().workflow_id.to_string())),
                        attribute("mf.run.id", string(run)),
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

fn send(endpoint: &str, path: &str, content_type: &str, body: &[u8]) -> u16 {
    let address = endpoint.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    let mut response = [0u8; 1024];
    let size = stream.read(&mut response).unwrap();
    std::str::from_utf8(&response[..size])
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn receives_protobuf_logs_and_traces_before_acknowledging() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let records = logs(vec![
        record(1, false, &run_id().to_string(), 1),
        record(2, true, &run_id().to_string(), 1),
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &records
        ),
        200
    );
    assert_eq!(
        receiver.snapshot().lifecycle.completeness,
        Completeness::Complete
    );
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/traces",
            "application/x-protobuf",
            &traces(&run_id().to_string())
        ),
        200
    );
    assert_eq!(receiver.snapshot().traces.observed_spans, 1);
    assert_eq!(
        receiver.finish().lifecycle.completeness,
        Completeness::Complete
    );
}

#[test]
fn unrelated_and_malformed_requests_are_visible_without_fabricating_events() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let mut unrelated_record = record(1, false, "87654321-4321-4321-9234-123456789abc", 1);
    unrelated_record.attributes.push(attribute(
        "other.bytes",
        AnyValue {
            value: Some(any_value::Value::BytesValue(vec![1, 2])),
        },
    ));
    let unrelated = logs(vec![unrelated_record]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &unrelated
        ),
        200
    );
    assert_eq!(receiver.snapshot().lifecycle.known_missing_count, 0);
    assert_eq!(receiver.snapshot().lifecycle.observation_errors, 0);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/traces",
            "application/x-protobuf",
            &traces("87654321-4321-4321-9234-123456789abc")
        ),
        200
    );
    assert_eq!(receiver.snapshot().traces.observed_spans, 0);
    let unsupported = logs(vec![record(1, false, &run_id().to_string(), 99)]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &unsupported
        ),
        400
    );
    assert_eq!(receiver.snapshot().lifecycle.local_drops, 1);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            b"\xff"
        ),
        400
    );
    assert_eq!(receiver.snapshot().lifecycle.observation_errors, 1);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/traces",
            "application/x-protobuf",
            b"\xff"
        ),
        400
    );
    assert_eq!(receiver.snapshot().traces.local_drops, 1);
    assert_eq!(receiver.snapshot().lifecycle.observation_errors, 1);
    let valid = logs(vec![
        record(1, false, &run_id().to_string(), 1),
        record(2, true, &run_id().to_string(), 1),
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &valid
        ),
        200
    );
    let snapshot = receiver.finish();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Incomplete);
    assert_eq!(snapshot.lifecycle.known_missing_count, 0);
    assert!(
        snapshot
            .diagnostics
            .iter()
            .any(|message| message.contains("schema"))
    );
}

#[test]
fn rejects_oversized_and_non_protobuf_requests() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    assert_eq!(
        send(receiver.endpoint(), "/v1/logs", "application/json", b"{}"),
        415
    );
    let address = receiver.endpoint().strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(stream, "POST /v1/logs HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_REQUEST_BYTES + 1).unwrap();
    let mut response = [0u8; 1024];
    let size = stream.read(&mut response).unwrap();
    assert!(
        std::str::from_utf8(&response[..size])
            .unwrap()
            .starts_with("HTTP/1.1 413")
    );
    assert_eq!(receiver.finish().lifecycle.observation_errors, 2);
}

#[test]
fn rejects_a_batch_above_the_record_budget_without_applying_a_prefix() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let unrelated = logs(vec![
        record(
            1,
            false,
            "87654321-4321-4321-9234-123456789abc",
            1
        );
        mf_tui::receiver::MAX_RECORDS_PER_REQUEST + 1
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &unrelated
        ),
        400
    );
    assert_eq!(receiver.snapshot().lifecycle.local_drops, 0);
    assert_eq!(receiver.snapshot().lifecycle.observation_errors, 0);
    let oversized = logs(vec![
        record(1, false, &run_id().to_string(), 1);
        mf_tui::receiver::MAX_RECORDS_PER_REQUEST + 1
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &oversized
        ),
        400
    );
    let snapshot = receiver.finish();
    assert_eq!(
        snapshot.lifecycle.local_drops,
        (mf_tui::receiver::MAX_RECORDS_PER_REQUEST + 1) as u64
    );
    assert_eq!(snapshot.lifecycle.observation_errors, 0);
    assert_eq!(
        snapshot.lifecycle.completeness,
        Completeness::UnverifiedTail
    );
    assert_eq!(snapshot.lifecycle.known_missing_count, 0);
}

#[test]
fn a_matching_event_over_the_admission_budget_is_counted_as_local_loss() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let mut oversized = record(2, true, &run_id().to_string(), 1);
    oversized
        .attributes
        .iter_mut()
        .find(|attribute| attribute.key == "mf.outcome")
        .unwrap()
        .value = Some(string("failed"));
    oversized
        .attributes
        .push(attribute("mf.failure.phase", string("preparation")));
    oversized.body = Some(AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: vec![
                attribute("final_sequence", integer(2)),
                attribute("elapsed_ns", integer(0)),
                attribute("visited_node_count", integer(0)),
                attribute(
                    "failure",
                    AnyValue {
                        value: Some(any_value::Value::KvlistValue(KeyValueList {
                            values: vec![attribute("message", string("x".repeat(129 * 1024)))],
                        })),
                    },
                ),
            ],
        })),
    });
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &logs(vec![oversized]),
        ),
        400
    );
    let snapshot = receiver.finish();
    assert_eq!(snapshot.lifecycle.local_drops, 1);
    assert_eq!(
        snapshot.lifecycle.completeness,
        Completeness::UnverifiedTail
    );
}
