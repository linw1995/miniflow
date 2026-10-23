use mf_runtime::{
    LiveOutputDecoder, LiveOutputEvent, LiveOutputKind, LiveOutputWriter, LiveProtocolError,
    LiveProtocolLimits, LiveRecord, LiveRecordKind, NodeValues, StreamCancellation,
    StreamInvocation, StreamOutputContract, ValueType,
};
use mf_telemetry::identity::RunId;
use std::sync::{Arc, Mutex};

fn schema() -> Vec<StreamOutputContract> {
    vec![StreamOutputContract {
        name: "answer".into(),
        path: vec!["produce".into()],
        port: "delta".into(),
        value_type: ValueType::String,
        required: true,
    }]
}

fn event(kind: LiveOutputKind) -> LiveOutputEvent {
    LiveOutputEvent {
        export: "answer".into(),
        source: vec!["produce".into()],
        invocation: StreamInvocation {
            id: 1,
            node: "produce".into(),
            path: vec![],
        },
        event: kind,
    }
}

fn wire(values: &[&str]) -> (RunId, Vec<u8>) {
    let run = RunId::new();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let output = bytes.clone();
    let writer = LiveOutputWriter::new(run, &schema(), Default::default(), move |bytes, _| {
        output.lock().unwrap().extend_from_slice(bytes);
        Ok(())
    })
    .unwrap();
    let publisher = writer.publisher();
    let cancellation = StreamCancellation::default();
    publisher
        .publish(event(LiveOutputKind::Open), &cancellation)
        .unwrap();
    for value in values {
        publisher
            .publish(
                event(LiveOutputKind::Value {
                    value: (*value).into(),
                }),
                &cancellation,
            )
            .unwrap();
    }
    publisher
        .publish(
            event(LiveOutputKind::Close { completed: true }),
            &cancellation,
        )
        .unwrap();
    writer
        .finish(
            &NodeValues::from([("text".into(), values.concat().into())]),
            &cancellation,
        )
        .unwrap();
    let bytes = bytes.lock().unwrap().clone();
    (run, bytes)
}

#[test]
fn records_round_trip_across_utf8_and_json_read_boundaries() {
    let (run, bytes) = wire(&["hello ", "caf\u{e9}\n"]);
    let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
    let mut events = Vec::new();
    for byte in &bytes {
        decoder
            .feed(std::slice::from_ref(byte), |event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
    }
    decoder.finish(|_| Ok(())).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        decoder.final_outputs().unwrap()["text"].as_str(),
        Some("hello caf\u{e9}\n")
    );
    assert!(matches!(events[0].event, LiveOutputKind::Open));
    assert!(matches!(
        events[3].event,
        LiveOutputKind::Close { completed: true }
    ));
}

#[test]
fn zero_values_and_never_invoked_exports_are_valid() {
    let (run, bytes) = wire(&[]);
    let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
    decoder.feed(&bytes, |_| Ok(())).unwrap();
    decoder.finish(|_| Ok(())).unwrap();
    let record = LiveRecord {
        version: 1,
        run_id: run.to_string(),
        sequence: 0,
        record: LiveRecordKind::Final {
            outputs: NodeValues::new(),
        },
    };
    let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
    decoder
        .feed(&serde_json::to_vec(&record).unwrap(), |_| Ok(()))
        .unwrap();
    decoder.finish(|_| Ok(())).unwrap();
    assert!(decoder.final_outputs().unwrap().is_empty());
}

#[test]
fn versions_sequences_sources_and_lifecycles_are_validated_before_final_acceptance() {
    let (run, bytes) = wire(&["value"]);
    let records = bytes
        .split(|byte| *byte == b'\n')
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).unwrap())
        .collect::<Vec<_>>();
    for case in [
        "version", "run", "sequence", "source", "value", "close", "final",
    ] {
        let mut changed = records.clone();
        match case {
            "version" => changed[0]["version"] = serde_json::json!(99),
            "run" => changed[0]["run_id"] = serde_json::json!(RunId::new().to_string()),
            "sequence" => changed[1]["sequence"] = serde_json::json!(9),
            "source" => changed[0]["record"]["event"]["source"] = serde_json::json!(["other"]),
            "value" => changed[1]["record"]["event"]["event"]["value"] = serde_json::json!(false),
            "close" => changed[2]["record"]["event"]["invocation"]["id"] = serde_json::json!(2),
            "final" => {
                changed.remove(2);
                changed[2]["sequence"] = serde_json::json!(2);
            }
            _ => unreachable!(),
        }
        let bytes = changed
            .iter()
            .flat_map(|record| {
                let mut bytes = serde_json::to_vec(record).unwrap();
                bytes.push(b'\n');
                bytes
            })
            .collect::<Vec<_>>();
        let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
        assert!(decoder.feed(&bytes, |_| Ok(())).is_err(), "{case}");
        assert!(decoder.final_outputs().is_none(), "{case}");
        assert!(
            decoder
                .feed(
                    &serde_json::to_vec(records.last().unwrap()).unwrap(),
                    |_| Ok(())
                )
                .is_err()
        );
    }
}

#[test]
fn duplicate_nested_keys_and_truncated_final_records_are_rejected() {
    let (run, bytes) = wire(&["value"]);
    let text = String::from_utf8(bytes.clone()).unwrap().replacen(
        "\"path\":[]",
        "\"path\":[],\"path\":[]",
        1,
    );
    let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
    assert!(matches!(
        decoder.feed(text.as_bytes(), |_| Ok(())),
        Err(LiveProtocolError::Json { .. })
    ));
    let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
    decoder.feed(&bytes[..bytes.len() - 3], |_| Ok(())).unwrap();
    assert!(decoder.finish(|_| Ok(())).is_err());
    assert!(decoder.final_outputs().is_none());
}

#[test]
fn a_valid_final_result_survives_later_diagnostics_in_the_same_read() {
    let run = RunId::new();
    let final_record = LiveRecord {
        version: 1,
        run_id: run.to_string(),
        sequence: 0,
        record: LiveRecordKind::Final {
            outputs: NodeValues::from([("text".into(), "complete".into())]),
        },
    };
    let mut final_bytes = serde_json::to_vec(&final_record).unwrap();
    final_bytes.push(b'\n');
    for limited in [false, true] {
        let limits = if limited {
            LiveProtocolLimits {
                max_event_bytes: 64,
                max_final_bytes: 64,
                max_total_bytes: final_bytes.len() + 2,
            }
        } else {
            Default::default()
        };
        let mut decoder = LiveOutputDecoder::new(run, &[], limits).unwrap();
        let bytes = [final_bytes.as_slice(), b"invalid late record\n"].concat();
        assert!(decoder.feed(&bytes, |_| Ok(())).is_err());
        assert_eq!(
            decoder.final_outputs().unwrap()["text"].as_str(),
            Some("complete")
        );
    }
}

#[test]
fn writer_limits_are_sticky_and_never_publish_a_final_after_failure() {
    let run = RunId::new();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let captured = bytes.clone();
    let writer = LiveOutputWriter::new(
        run,
        &schema(),
        LiveProtocolLimits {
            max_event_bytes: 512,
            max_final_bytes: 512,
            max_total_bytes: 4096,
        },
        move |bytes, _| {
            captured.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        },
    )
    .unwrap();
    let publisher = writer.publisher();
    let cancellation = StreamCancellation::default();
    publisher
        .publish(event(LiveOutputKind::Open), &cancellation)
        .unwrap();
    assert!(
        publisher
            .publish(
                event(LiveOutputKind::Value {
                    value: "x".repeat(1024).into()
                }),
                &cancellation
            )
            .is_err()
    );
    assert!(writer.finish(&NodeValues::new(), &cancellation).is_err());
    let bytes = bytes.lock().unwrap();
    assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
}

#[test]
fn invocation_identity_cannot_change_between_open_values_and_close() {
    for kind in [
        LiveOutputKind::Value {
            value: "preview".into(),
        },
        LiveOutputKind::Close { completed: true },
    ] {
        let run = RunId::new();
        let records = [event(LiveOutputKind::Open), {
            let mut changed = event(kind);
            changed
                .invocation
                .path
                .push(mf_telemetry::event::LoopPathEntry {
                    loop_id: "nested".into(),
                    index: mf_telemetry::Count::try_from(1).unwrap(),
                });
            changed
        }];
        let mut decoder = LiveOutputDecoder::new(run, &schema(), Default::default()).unwrap();
        for (sequence, event) in records.into_iter().enumerate() {
            let mut bytes = serde_json::to_vec(&LiveRecord {
                version: 1,
                run_id: run.to_string(),
                sequence: sequence as u64,
                record: LiveRecordKind::Event { event },
            })
            .unwrap();
            bytes.push(b'\n');
            let result = decoder.feed(&bytes, |_| Ok(()));
            if sequence == 0 {
                result.unwrap();
            } else {
                assert!(matches!(result, Err(LiveProtocolError::Lifecycle { .. })));
            }
        }
        assert!(decoder.final_outputs().is_none());
        assert!(decoder.finish(|_| Ok(())).is_err());
    }
}

#[test]
fn delivery_failures_preserve_sources_and_prevent_further_writes() {
    use std::error::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let captured = attempts.clone();
    let writer = LiveOutputWriter::new(RunId::new(), &schema(), Default::default(), move |_, _| {
        captured.fetch_add(1, Ordering::SeqCst);
        Err(mf_runtime::StreamError::Execution {
            message: "transport disconnected".into(),
        })
    })
    .unwrap();
    let cancellation = StreamCancellation::default();
    assert!(
        writer
            .publisher()
            .publish(event(LiveOutputKind::Open), &cancellation)
            .is_err()
    );
    let error = writer
        .finish(&NodeValues::new(), &cancellation)
        .unwrap_err();
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<Arc<mf_runtime::StreamError>>()
        .unwrap();
    assert!(
        matches!(source.as_ref(), mf_runtime::StreamError::Execution { message } if message == "transport disconnected")
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}
