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
        injected_bodies: Vec::new(),
        order_edges: Vec::new(),
        version: WorkflowDescriptionVersion::V2026_09_27,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
        nodes: vec![],
        data_edges: vec![],
        control_edges: vec![],
        execution_order: vec![],
        execution: None,
        loop_bodies: Vec::new(),
        clusters: Vec::new(),
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
        body: Some(body(&[("elapsed_ns", 0)])),
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
fn graph_metrics_use_source_events_before_snapshots_and_keep_call_identity() {
    use mf_telemetry::description::NodeDescription;
    use mf_tui::graph::{GraphLayout, GraphView};
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    if !loopback_available() {
        return;
    }
    let mut graph = graph();
    graph.nodes.push(NodeDescription {
        id: "model".into(),
        kind: "builtin.llm".into(),
    });
    graph.execution_order.push("model".into());
    let layout = GraphLayout::new(&graph).unwrap();
    let mut receiver = LoopbackReceiver::bind(graph.clone(), run_id()).unwrap();
    let mut started = record(
        2,
        false,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    started.event_name = "mf.node.started".into();
    started.attributes.extend([
        attribute("mf.node.id", string("model")),
        attribute("mf.node.kind", string("builtin.llm")),
    ]);
    started.body = Some(body(&[("position", 0), ("elapsed_ns", 0)]));
    started.trace_id = vec![1; 16];
    started.span_id = vec![2; 8];
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &logs(vec![started])
        ),
        200
    );
    let reader = receiver.reader();
    let render = || {
        let snapshot = reader.snapshot_shared();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 20));
        GraphView::new(&layout)
            .snapshot(&snapshot)
            .reader(&reader)
            .render(buffer.area, &mut buffer);
        (0..20)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(render().contains("FTT —"));
    let usage =
        serde_json::json!({"call_id":"answer", "model":"fixture", "usage":{"output_tokens":100}});
    let mut call_events = vec![];
    for (name, timestamp, with_usage) in [
        ("llm.request.finished", 12_000_000_000, true),
        ("llm.response.first_text", 10_500_000_000, false),
        ("llm.request.started", 10_000_000_000, true),
    ] {
        let mut fields = vec![attribute("call_id", string("answer"))];
        if with_usage {
            fields.push(attribute("model", string("fixture")));
            fields.push(attribute("usage", body(&[("output_tokens", 100)])));
        }
        call_events.push(LogRecord {
            event_name: name.into(),
            time_unix_nano: timestamp,
            observed_time_unix_nano: 90_000_000_000,
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            attributes: vec![
                attribute("mf.workflow.id", string(graph.workflow_id.as_str())),
                attribute("mf.run.id", string(run_id().to_string())),
            ],
            body: Some(AnyValue {
                value: Some(any_value::Value::KvlistValue(KeyValueList {
                    values: fields,
                })),
            }),
            ..Default::default()
        });
    }
    let mut request = ExportLogsServiceRequest::decode(logs(call_events).as_slice()).unwrap();
    request.resource_logs[0].scope_logs[0]
        .scope
        .as_mut()
        .unwrap()
        .name = mf_telemetry::events::SCOPE.into();
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec()
        ),
        200
    );
    let text = render();
    assert!(text.contains("FTT 500ms"), "{text}");
    assert!(text.contains("Gen 66.7 | E2E 50.0 tok/s"), "{text}");
    let bodies = [
        (
            vec![2; 8],
            serde_json::json!({"record":"header","version":mf_telemetry::capture::SNAPSHOT_VERSION}),
        ),
        (
            vec![2; 8],
            serde_json::json!({"record":"node","scope":[],"node":"model","inputs":{},"outputs":{"usages":[usage]},"skipped":[],"outcome":"succeeded"}),
        ),
        (
            vec![3; 8],
            serde_json::json!({"record":"node","scope":[],"node":"model","inputs":{},"outputs":{"usages":[]},"skipped":[],"outcome":"started"}),
        ),
        (vec![3; 8], serde_json::json!({"record":"end"})),
    ];
    let records = bodies
        .into_iter()
        .enumerate()
        .flat_map(|(sequence, (span, body))| {
            let workflow_id = graph.workflow_id.clone();
            mf_telemetry::snapshot::packets(sequence, body)
                .unwrap()
                .map(move |packet| {
                    let mut record =
                        packet.into_log_record(workflow_id.as_str(), &run_id().to_string());
                    record.trace_id = vec![1; 16];
                    record.span_id = span.clone();
                    record
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut request = ExportLogsServiceRequest::decode(logs(records).as_slice()).unwrap();
    request.resource_logs[0].scope_logs[0]
        .scope
        .as_mut()
        .unwrap()
        .name = mf_telemetry::snapshot::SCOPE.into();
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec()
        ),
        200
    );
    let text = render();
    assert!(text.contains("FTT 500ms"), "{text}");
    assert!(text.contains("Gen 66.7 | E2E 50.0 tok/s"), "{text}");
    receiver.finish();
    assert!(render().contains("FTT 500ms"));
}

#[test]
fn receives_protobuf_logs_and_traces_before_acknowledging() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let mut started = record(
        1,
        false,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    started.attributes.push(attribute(
        "future.bytes",
        AnyValue {
            value: Some(any_value::Value::BytesValue(vec![1, 2])),
        },
    ));
    for index in 0..70 {
        started
            .attributes
            .push(attribute(&format!("future.{index}"), string("ignored")));
    }
    let records = logs(vec![
        started,
        record(
            2,
            true,
            &run_id().to_string(),
            mf_telemetry::EVENT_SCHEMA_VERSION,
        ),
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
fn iteration_detail_logs_leave_outer_lifecycle_complete() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let detail = LogRecord {
        event_name: "mf.iteration.node.finished".into(),
        attributes: vec![
            attribute("mf.workflow.id", string(graph().workflow_id.to_string())),
            attribute("mf.run.id", string(run_id().to_string())),
            attribute("mf.iteration.id", string("iteration")),
            attribute("mf.iteration.index", integer(0)),
            attribute("mf.node.id", string("body")),
        ],
        body: Some(string("mf.iteration.node.finished")),
        ..Default::default()
    };
    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![
                ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "mf.iteration".into(),
                        ..Default::default()
                    }),
                    log_records: vec![detail],
                    ..Default::default()
                },
                ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "mf.workflow".into(),
                        ..Default::default()
                    }),
                    log_records: vec![
                        record(
                            1,
                            false,
                            &run_id().to_string(),
                            mf_telemetry::EVENT_SCHEMA_VERSION,
                        ),
                        record(
                            2,
                            true,
                            &run_id().to_string(),
                            mf_telemetry::EVENT_SCHEMA_VERSION,
                        ),
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request,
        ),
        200
    );
    let snapshot = receiver.finish();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
    assert_eq!(snapshot.lifecycle.local_drops, 0);
    assert_eq!(snapshot.observation.errors, 0);
}

fn observed_iteration_graph() -> WorkflowDescription {
    serde_json::from_value(serde_json::json!({
        "version":"2026-10-23", "workflow_id":graph().workflow_id,
        "execution":{"mode":"single"}, "nodes":[{"id":"each","kind":"builtin.iteration"}],
        "execution_order":["each"], "data_edges":[],"control_edges":[],
        "loop_bodies":[{"path":["each"],"nodes":[{"id":"%iteration","kind":"builtin.iteration_input"},{"id":"work","kind":"fixture.work"}],
            "execution_order":["%iteration","work"],"data_edges":[],"control_edges":[]}]
    }))
    .unwrap()
}

fn iteration_log(sequence: i64, name: &str, extra: Vec<KeyValue>) -> LogRecord {
    use mf_telemetry::{
        Count,
        iteration::{CONTEXT_ATTRIBUTE, IterationContext},
    };
    let context = IterationContext {
        invocation: Count::try_from(1).unwrap(),
        iteration_path: vec!["each".into()],
        body_path: vec!["each".into()],
        parent_path: Vec::new(),
        parent_stream_invocation: None,
        index: Count::ZERO,
        sequence: Count::try_from(sequence).unwrap(),
        elapsed_ns: Count::try_from(sequence).unwrap(),
    };
    let mut attributes = vec![
        attribute("mf.workflow.id", string(graph().workflow_id.to_string())),
        attribute("mf.run.id", string(run_id().to_string())),
        attribute("mf.iteration.id", string("each")),
        attribute("mf.iteration.index", integer(0)),
        attribute(
            CONTEXT_ATTRIBUTE,
            string(serde_json::to_string(&context).unwrap()),
        ),
        attribute("mf.node.id", string("work")),
        attribute("mf.node.kind", string("fixture.work")),
    ];
    attributes.extend(extra);
    LogRecord {
        event_name: name.into(),
        attributes,
        body: Some(string(name)),
        ..Default::default()
    }
}

fn iteration_request(records: Vec<LogRecord>) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "mf.iteration".into(),
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

fn iteration_array(values: Vec<AnyValue>) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::ArrayValue(
            opentelemetry_proto::tonic::common::v1::ArrayValue { values },
        )),
    }
}

#[test]
fn iteration_failure_and_skip_logs_keep_structured_node_evidence() {
    use mf_tui::state::NodeStatus;
    if !loopback_available() {
        return;
    }
    for skipped in [false, true] {
        let receiver = LoopbackReceiver::bind(observed_iteration_graph(), run_id()).unwrap();
        let mut records = vec![iteration_log(1, "mf.iteration.item.started", Vec::new())];
        if skipped {
            records.push(iteration_log(
                2,
                "mf.iteration.node.skipped",
                vec![
                    attribute(
                        "mf.skip.causes",
                        iteration_array(vec![AnyValue {
                            value: Some(any_value::Value::KvlistValue(KeyValueList {
                                values: vec![
                                    attribute("source_node", string("%iteration")),
                                    attribute("source_output", string("item")),
                                ],
                            })),
                        }]),
                    ),
                    attribute("mf.skipped.ports", iteration_array(vec![string("value")])),
                ],
            ));
        } else {
            records.push(iteration_log(
                2,
                "mf.iteration.node.started",
                vec![attribute(
                    "mf.input.selections",
                    iteration_array(vec![AnyValue {
                        value: Some(any_value::Value::KvlistValue(KeyValueList {
                            values: vec![
                                attribute("input", string("input")),
                                attribute("source_node", string("%iteration")),
                                attribute("source_output", string("item")),
                            ],
                        })),
                    }]),
                )],
            ));
            records.push(iteration_log(
                3,
                "mf.iteration.node.finished",
                vec![
                    attribute("mf.outcome", string("failed")),
                    attribute("mf.failure.phase", string("execution")),
                    attribute("mf.failure.message", string("fixture failure")),
                    attribute("mf.duration.ns", integer(9)),
                ],
            ));
        }
        let payload = iteration_request(records);
        assert_eq!(
            send(
                receiver.endpoint(),
                "/v1/logs",
                "application/x-protobuf",
                &payload
            ),
            200
        );
        let snapshot = receiver.snapshot();
        let node = &snapshot.iterations.items[0].nodes[0].node;
        assert_eq!(
            node.status,
            if skipped {
                NodeStatus::Skipped
            } else {
                NodeStatus::Failed
            }
        );
        if skipped {
            assert_eq!(node.skip_causes.len(), 1);
            assert_eq!(node.skipped_ports, ["value"]);
        } else {
            assert_eq!(node.failure.as_ref().unwrap().message, "fixture failure");
            assert_eq!(node.input_selections[0].source_node, "%iteration");
            assert_eq!(node.duration_ns.unwrap().get(), 9);
        }
        assert_eq!(snapshot.observation.errors, 0);
        assert_eq!(snapshot.lifecycle.known_missing_count, 0);
    }
}

#[test]
fn malformed_iteration_details_are_rejected_without_lifecycle_loss() {
    if !loopback_available() {
        return;
    }
    let receiver = LoopbackReceiver::bind(observed_iteration_graph(), run_id()).unwrap();
    let valid = iteration_log(1, "mf.iteration.node.started", Vec::new());
    for scenario in 0..7 {
        let mut record = valid.clone();
        match scenario {
            0 => record.attributes[1].value = Some(string(RunId::new().to_string())),
            1 => record.attributes[2].value = None,
            2 => record.attributes[3].value = Some(integer(9)),
            3 => record
                .attributes
                .retain(|attribute| attribute.key != "mf.node.id"),
            4 => record.trace_id = vec![1],
            5 => {
                record.event_name = "mf.iteration.node.finished".into();
                record
                    .attributes
                    .push(attribute("mf.outcome", string("failed")));
            }
            _ => record.event_name = "fixture.unknown".into(),
        }
        let expected = if scenario == 0 || scenario == 6 {
            200
        } else {
            400
        };
        assert_eq!(
            send(
                receiver.endpoint(),
                "/v1/logs",
                "application/x-protobuf",
                &iteration_request(vec![record])
            ),
            expected,
            "scenario {scenario}"
        );
    }
    assert!(receiver.snapshot().iterations.items.is_empty());
    assert_eq!(receiver.snapshot().lifecycle.local_drops, 0);
}

#[test]
fn iteration_nodes_update_live_without_spans_or_outer_lifecycle_records() {
    use mf_tui::state::NodeStatus;
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(observed_iteration_graph(), run_id()).unwrap();
    let initial = receiver.snapshot().lifecycle;
    let payload = iteration_request(vec![
        iteration_log(1, "mf.iteration.item.started", Vec::new()),
        iteration_log(2, "mf.iteration.node.started", Vec::new()),
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &payload
        ),
        200
    );
    let snapshot = receiver.snapshot();
    assert_eq!(
        snapshot.iterations.items[0].nodes[0].node.status,
        NodeStatus::Running
    );
    assert_eq!(snapshot.traces.observed_spans, 0);
    assert_eq!(snapshot.lifecycle, initial);
    let payload = iteration_request(vec![
        iteration_log(
            3,
            "mf.iteration.node.finished",
            vec![
                attribute("mf.outcome", string("succeeded")),
                attribute("mf.duration.ns", integer(9)),
            ],
        ),
        iteration_log(
            4,
            "mf.iteration.item.finished",
            vec![attribute("mf.outcome", string("succeeded"))],
        ),
        iteration_log(5, "mf.iteration.finished", Vec::new()),
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &payload
        ),
        200
    );
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &payload
        ),
        200
    );
    let snapshot = receiver.snapshot();
    assert!(snapshot.iterations.items[0].closed);
    assert_eq!(
        snapshot.iterations.items[0].nodes[0].node.status,
        NodeStatus::Succeeded
    );
    assert_eq!(snapshot.iterations.protocol_conflicts, 0);
    assert_eq!(snapshot.iterations.known_missing_count, 0);
    assert_eq!(snapshot.lifecycle, initial);
    receiver.finish();
}

#[test]
fn raw_log_retention_does_not_stop_iteration_state_reduction() {
    use mf_tui::state::NodeStatus;
    if !loopback_available() {
        return;
    }
    let receiver = LoopbackReceiver::bind(observed_iteration_graph(), run_id()).unwrap();
    for batch in 0..4 {
        let payload = iteration_request(
            (0..1024)
                .map(|index| LogRecord {
                    event_name: "fixture.detail".into(),
                    attributes: vec![
                        attribute("mf.workflow.id", string(graph().workflow_id.to_string())),
                        attribute("mf.run.id", string(run_id().to_string())),
                        attribute("fixture.index", integer(batch * 1024 + index)),
                    ],
                    ..Default::default()
                })
                .collect(),
        );
        assert_eq!(
            send(
                receiver.endpoint(),
                "/v1/logs",
                "application/x-protobuf",
                &payload
            ),
            200
        );
    }
    let payload = iteration_request(vec![
        iteration_log(1, "mf.iteration.item.started", Vec::new()),
        iteration_log(2, "mf.iteration.node.started", Vec::new()),
    ]);
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &payload
        ),
        200
    );
    assert_eq!(receiver.log_snapshot().records.len(), 4096);
    assert_eq!(receiver.log_snapshot().local_drops, 2);
    assert_eq!(
        receiver.snapshot().iterations.items[0].nodes[0].node.status,
        NodeStatus::Running
    );
    let mut invalid = iteration_log(
        3,
        "mf.iteration.node.finished",
        vec![attribute("mf.outcome", string("succeeded"))],
    );
    invalid
        .attributes
        .push(attribute("mf.node.id", string("conflicting")));
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &iteration_request(vec![invalid])
        ),
        400
    );
    assert_eq!(
        receiver.snapshot().iterations.items[0].nodes[0].node.status,
        NodeStatus::Running
    );
    assert!(receiver.snapshot().observation.errors > 0);
}

#[test]
fn unrelated_and_malformed_requests_are_visible_without_fabricating_events() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let mut unrelated_record = record(
        1,
        false,
        "87654321-4321-4321-9234-123456789abc",
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
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
    assert_eq!(receiver.snapshot().observation.errors, 0);
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
    assert_eq!(receiver.snapshot().observation.errors, 1);
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
    assert_eq!(receiver.snapshot().observation.errors, 1);
    let valid = logs(vec![
        record(
            1,
            false,
            &run_id().to_string(),
            mf_telemetry::EVENT_SCHEMA_VERSION,
        ),
        record(
            2,
            true,
            &run_id().to_string(),
            mf_telemetry::EVENT_SCHEMA_VERSION,
        ),
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
    assert_eq!(receiver.finish().observation.errors, 2);
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
            mf_telemetry::EVENT_SCHEMA_VERSION
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
    assert_eq!(receiver.snapshot().observation.errors, 0);
    let oversized = logs(vec![
        record(
            1,
            false,
            &run_id().to_string(),
            mf_telemetry::EVENT_SCHEMA_VERSION
        );
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
    assert_eq!(snapshot.observation.errors, 0);
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
    let mut oversized = record(
        2,
        true,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
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
                attribute("elapsed_ns", integer(0)),
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

#[test]
fn retains_correlated_trace_details_after_completion_and_deduplicates_exports() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let bytes = traces(&run_id().to_string());
    let mut request = ExportTraceServiceRequest::decode(bytes.as_slice()).unwrap();
    let resource = &mut request.resource_spans[0];
    resource.resource = Some(opentelemetry_proto::tonic::resource::v1::Resource {
        attributes: vec![attribute("service.name", string("test-workflow"))],
        ..Default::default()
    });
    resource.scope_spans[0].scope = Some(InstrumentationScope {
        name: "mf.workflow".into(),
        version: "test".into(),
        ..Default::default()
    });
    let span = &mut resource.scope_spans[0].spans[0];
    span.start_time_unix_nano = 10;
    span.end_time_unix_nano = 52;
    span.attributes
        .push(attribute("custom.attribute", string("trace-detail")));
    let bytes = request.encode_to_vec();
    for _ in 0..2 {
        assert_eq!(
            send(
                receiver.endpoint(),
                "/v1/traces",
                "application/x-protobuf",
                &bytes
            ),
            200
        );
    }
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/traces",
            "application/x-protobuf",
            &traces("87654321-4321-4321-9234-123456789abc")
        ),
        200
    );
    let snapshot = receiver.trace_snapshot();
    assert_eq!(snapshot.rows.len(), 1);
    let observation = &snapshot.rows[0].observation;
    assert_eq!(observation.span.start_time_unix_nano, 10);
    assert_eq!(observation.span.end_time_unix_nano, 52);
    assert_eq!(
        observation.span.attributes.last().unwrap().key,
        "custom.attribute"
    );
    assert_eq!(
        observation.resource.as_ref().unwrap().attributes[0].key,
        "service.name"
    );
    assert_eq!(observation.scope.as_ref().unwrap().version, "test");
    receiver.finish();
    assert_eq!(receiver.trace_snapshot().rows.len(), 1);
    assert_eq!(receiver.snapshot().traces.local_drops, 0);
}

#[test]
fn rejects_invalid_trace_identity_parent_and_timestamps_without_lifecycle_loss() {
    if !loopback_available() {
        return;
    }
    let receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    for invalid in 0..4 {
        let bytes = traces(&run_id().to_string());
        let mut request = ExportTraceServiceRequest::decode(bytes.as_slice()).unwrap();
        let span = &mut request.resource_spans[0].scope_spans[0].spans[0];
        match invalid {
            0 => span.trace_id = vec![0; 16],
            1 => span.span_id = vec![0; 8],
            2 => span.parent_span_id = vec![1; 7],
            _ => {
                span.start_time_unix_nano = 2;
                span.end_time_unix_nano = 1;
            }
        }
        assert_eq!(
            send(
                receiver.endpoint(),
                "/v1/traces",
                "application/x-protobuf",
                &request.encode_to_vec()
            ),
            400
        );
    }
    assert!(receiver.trace_snapshot().rows.is_empty());
    let snapshot = receiver.snapshot();
    assert_eq!(snapshot.traces.local_drops, 4);
    assert_eq!(snapshot.lifecycle.local_drops, 0);
}

#[test]
fn retains_session_logs_before_spans_and_ignores_other_runs() {
    if !loopback_available() {
        return;
    }
    let receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let mut record = record(
        1,
        false,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    record.event_name = "custom.log".into();
    record.trace_id = vec![1; 16];
    record.span_id = vec![2; 8];
    let mut request =
        ExportLogsServiceRequest::decode(logs(vec![record.clone()]).as_slice()).unwrap();
    request.resource_logs[0].scope_logs[0]
        .scope
        .as_mut()
        .unwrap()
        .name = "custom.scope".into();
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec()
        ),
        200
    );
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec()
        ),
        200
    );
    let mut unrelated = record.clone();
    unrelated
        .attributes
        .iter_mut()
        .find(|attribute| attribute.key == "mf.run.id")
        .unwrap()
        .value = Some(string("87654321-4321-4321-9234-123456789abc"));
    request.resource_logs[0].scope_logs[0].log_records = vec![unrelated];
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec()
        ),
        200
    );
    assert!(receiver.trace_snapshot().rows.is_empty());
    let retained = receiver.log_snapshot();
    assert_eq!(retained.records.len(), 1);
    assert_eq!(retained.observed_records, 2);
    assert_eq!(retained.related(&[1; 16], &[2; 8]).len(), 1);
    assert_eq!(
        retained.records[0].scope.as_ref().unwrap().name,
        "custom.scope"
    );
    assert_eq!(receiver.snapshot().observation.errors, 0);
}

#[test]
fn raw_log_limits_do_not_stop_lifecycle_reduction_or_snapshot_assembly() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    for first in (0..4096).step_by(512) {
        let records = (first..first + 512)
            .map(|index| {
                let mut record = record(
                    1,
                    false,
                    &run_id().to_string(),
                    mf_telemetry::EVENT_SCHEMA_VERSION,
                );
                record.time_unix_nano = index as u64 + 1;
                record.event_name = "custom.log".into();
                record
            })
            .collect();
        let mut request = ExportLogsServiceRequest::decode(logs(records).as_slice()).unwrap();
        request.resource_logs[0].scope_logs[0]
            .scope
            .as_mut()
            .unwrap()
            .name = "custom.scope".into();
        assert_eq!(
            send(
                receiver.endpoint(),
                "/v1/logs",
                "application/x-protobuf",
                &request.encode_to_vec()
            ),
            200
        );
    }
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &logs(vec![
                record(
                    1,
                    false,
                    &run_id().to_string(),
                    mf_telemetry::EVENT_SCHEMA_VERSION
                ),
                record(
                    2,
                    true,
                    &run_id().to_string(),
                    mf_telemetry::EVENT_SCHEMA_VERSION
                )
            ])
        ),
        200
    );
    let bodies = [
        serde_json::json!({"record":"header","version":mf_telemetry::capture::SNAPSHOT_VERSION}),
        serde_json::json!({"record":"node","scope":[],"node":"step","inputs":{},"outputs":{},"skipped":[],"outcome":"succeeded"}),
        serde_json::json!({"record":"end"}),
    ];
    let records: Vec<_> = bodies
        .into_iter()
        .enumerate()
        .flat_map(|(sequence, body)| {
            mf_telemetry::snapshot::packets(sequence, body)
                .unwrap()
                .map(|packet| {
                    let mut record =
                        packet.into_log_record(graph().workflow_id.as_str(), &run_id().to_string());
                    record.trace_id = vec![1; 16];
                    record.span_id = vec![2; 8];
                    record
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let expected_drops = 2 + records.len() as u64;
    let mut request = ExportLogsServiceRequest::decode(logs(records).as_slice()).unwrap();
    request.resource_logs[0].scope_logs[0]
        .scope
        .as_mut()
        .unwrap()
        .name = mf_telemetry::snapshot::SCOPE.into();
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec()
        ),
        200
    );
    let state = receiver.finish();
    assert_eq!(state.lifecycle.completeness, Completeness::Complete);
    assert_eq!(state.lifecycle.local_drops, 0);
    assert_eq!(state.observation.errors, 0);
    assert_eq!(receiver.log_snapshot().records.len(), 4096);
    assert_eq!(receiver.log_snapshot().local_drops, expected_drops);
    let data = receiver.span_data(&[1; 16], &[2; 8]);
    assert!(data.node.is_some());
    assert_eq!(data.status, "Complete");
}

#[test]
fn call_events_ignore_foreign_records_and_reject_malformed_source_metadata() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let submit = |records| {
        let mut request = ExportLogsServiceRequest::decode(logs(records).as_slice()).unwrap();
        request.resource_logs[0].scope_logs[0]
            .scope
            .as_mut()
            .unwrap()
            .name = mf_telemetry::events::SCOPE.into();
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &request.encode_to_vec(),
        )
    };
    let mut started = record(
        0,
        false,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    started.event_name = "llm.request.started".into();
    started.time_unix_nano = 1_000_000_000;
    started.body = Some(AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: vec![
                attribute("call_id", string("call")),
                attribute("model", string("fixture")),
            ],
        })),
    });
    let mut finished = started.clone();
    finished.event_name = "llm.request.finished".into();
    finished.time_unix_nano = 3_000_000_000;
    assert_eq!(submit(vec![finished, started.clone()]), 200);
    for key in ["mf.run.id", "mf.workflow.id"] {
        let mut foreign = started.clone();
        foreign
            .attributes
            .iter_mut()
            .find(|attribute| attribute.key == key)
            .unwrap()
            .value = Some(string("foreign"));
        foreign.body = None;
        assert_eq!(submit(vec![foreign]), 200);
    }
    let mut unrelated = started.clone();
    unrelated.event_name = "future.observation".into();
    unrelated.body = None;
    assert_eq!(submit(vec![unrelated]), 200);
    let mut missing = started.clone();
    missing.body = None;
    let mut wrong_width = started.clone();
    wrong_width.trace_id = vec![1];
    let mut zero_context = started.clone();
    zero_context.trace_id = vec![0; 16];
    zero_context.span_id = vec![0; 8];
    let mut wrong_body = started.clone();
    wrong_body.body = Some(string("invalid payload"));
    let mut missing_timestamp = started;
    missing_timestamp.time_unix_nano = 0;
    missing_timestamp.observed_time_unix_nano = 99_000_000_000;
    for malformed in [
        missing,
        wrong_width,
        zero_context,
        wrong_body,
        missing_timestamp,
    ] {
        assert_eq!(submit(vec![malformed]), 400);
    }
    let mut usages = vec![
        serde_json::from_value(serde_json::json!({"call_id":"call", "model":"fixture"})).unwrap(),
    ];
    receiver.reader().apply_call_timings(&mut usages);
    assert_eq!(usages[0].timing.as_ref().unwrap().elapsed_us, 2_000_000);
    assert_eq!(usages[0].timing.as_ref().unwrap().first_token_us, None);
    assert!(
        receiver
            .reader()
            .calls_for_span(&[1; 16], &[2; 8])
            .is_empty()
    );
    receiver.finish();
}

#[test]
fn business_validation_errors_preserve_delivered_sequence_evidence() {
    if !loopback_available() {
        return;
    }
    let mut receiver = LoopbackReceiver::bind(graph(), run_id()).unwrap();
    let start = record(
        1,
        false,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &logs(vec![start])
        ),
        200
    );
    let mut unknown = record(
        2,
        false,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    unknown.event_name = "mf.node.started".into();
    unknown.attributes.extend([
        attribute("mf.node.id", string("unknown.node")),
        attribute("mf.node.kind", string("fixture.task")),
    ]);
    unknown.body = Some(body(&[("position", 0), ("elapsed_ns", 1)]));
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &logs(vec![unknown])
        ),
        400
    );
    let end = record(
        3,
        true,
        &run_id().to_string(),
        mf_telemetry::EVENT_SCHEMA_VERSION,
    );
    assert_eq!(
        send(
            receiver.endpoint(),
            "/v1/logs",
            "application/x-protobuf",
            &logs(vec![end])
        ),
        200
    );
    let snapshot = receiver.finish();
    assert_eq!(snapshot.lifecycle.completeness, Completeness::Complete);
    assert_eq!(snapshot.lifecycle.known_missing_count, 0);
    assert_eq!(snapshot.lifecycle.local_drops, 0);
    assert_eq!(snapshot.observation.errors, 1);
}
