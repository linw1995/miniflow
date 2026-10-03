use mf_telemetry::{
    Count,
    event::{Event, NodeIdentity},
    identity::{RunId, WorkflowId},
    stream::{StreamIdentity, StreamMessage, StreamPayload, StreamRecord, StreamTrigger},
};
use serde_json::json;

fn record() -> StreamRecord {
    StreamRecord {
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
