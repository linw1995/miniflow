use mf_telemetry::{
    Count,
    event::{Event, NodeIdentity},
    identity::{RunId, WorkflowId},
    stream::{StreamIdentity, StreamMessage, StreamPayload, StreamRecord, StreamTrigger},
};
use serde_json::json;

fn record() -> StreamRecord {
    StreamRecord {
        schema_version: mf_telemetry::STREAM_EVENT_SCHEMA_VERSION,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
        run_id: RunId::new(),
        sequence: Count::try_from(50_000).unwrap(),
        identity: Some(StreamIdentity {
            invocation: u64::MAX - 1,
            parent: Some(u64::MAX - 2),
            trigger: StreamTrigger::Message,
            message: Some(StreamMessage {
                domain: 2,
                sequence: u64::MAX - 1,
            }),
        }),
        payload: StreamPayload::Execution(Event::NodeStarted {
            node: NodeIdentity {
                id: "consume".into(),
                kind: "builtin.identity".into(),
                path: Vec::new(),
            },
            position: Count::try_from(2).unwrap(),
            elapsed_ns: Count::try_from(10).unwrap(),
        }),
        emission_count: None,
    }
}

#[test]
fn identities_round_trip_without_numeric_precision_loss_or_legacy_event_bounds() {
    let record = record();
    let wire = record.to_wire(1, None).unwrap();
    assert_eq!(
        wire.body["stream"]["invocation"],
        (u64::MAX - 1).to_string()
    );
    assert_eq!(StreamRecord::decode(&wire).unwrap(), record);
    assert!(wire.decode().is_err());
}

#[test]
fn malformed_stream_identities_and_cross_protocol_records_are_rejected() {
    let record = record();
    let wire = record.to_wire(1, None).unwrap();
    for invalid in [
        json!("01"),
        json!("-1"),
        json!(1),
        json!("18446744073709551616"),
    ] {
        let mut changed = wire.clone();
        changed.body["stream"]["invocation"] = invalid;
        assert!(StreamRecord::decode(&changed).is_err());
    }
    let mut changed = wire.clone();
    changed.body["stream"]["trigger"] = json!("timer");
    assert!(StreamRecord::decode(&changed).is_err());
    let mut changed = wire.clone();
    changed.body.as_object_mut().unwrap().remove("stream");
    assert!(StreamRecord::decode(&changed).is_err());
    let mut changed = wire;
    changed.attributes["mf.schema.version"] = json!(2);
    assert!(StreamRecord::decode(&changed).is_err());
}

#[test]
fn startup_records_have_no_external_message_and_legacy_records_keep_their_version() {
    let mut startup = record();
    startup.identity.as_mut().unwrap().trigger = StreamTrigger::Startup;
    startup.identity.as_mut().unwrap().message = None;
    let wire = startup.to_wire(1, None).unwrap();
    assert_eq!(StreamRecord::decode(&wire).unwrap(), startup);
    startup.schema_version = mf_telemetry::LEGACY_STREAM_EVENT_SCHEMA_VERSION;
    assert!(startup.to_wire(1, None).is_err());
    let mut legacy = record();
    legacy.schema_version = mf_telemetry::LEGACY_STREAM_EVENT_SCHEMA_VERSION;
    let wire = legacy.to_wire(1, None).unwrap();
    assert_eq!(StreamRecord::decode(&wire).unwrap(), legacy);
    assert_eq!(wire.attributes["mf.schema.version"], json!(3));
}

#[test]
fn terminal_counters_are_validated_and_serialized_for_their_protocol() {
    use mf_telemetry::stream::{StreamCounts, StreamEvent, StreamOutcome};
    let mut terminal = record();
    terminal.identity = None;
    terminal.payload = StreamPayload::Control(StreamEvent::Finished {
        final_sequence: terminal.sequence,
        elapsed_ns: Count::try_from(100).unwrap(),
        outcome: StreamOutcome::Succeeded,
        counts: StreamCounts {
            startup_frames: 1,
            emitted_messages: 2,
            completed_frames: 3,
            delivered_outputs: 2,
            ..Default::default()
        },
        failure: None,
    });
    let wire = terminal.to_wire(1, None).unwrap();
    assert_eq!(wire.body["counts"]["startup_frames"], json!("1"));
    assert!(wire.body["counts"].get("accepted_inputs").is_none());
    assert_eq!(StreamRecord::decode(&wire).unwrap(), terminal);
    let mut invalid = wire;
    invalid.body["counts"]["startup_frames"] = json!("2");
    assert!(StreamRecord::decode(&invalid).is_err());
}
