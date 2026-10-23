use mf_runtime::{
    LiveLimits, LiveOutputEvent, LiveOutputKind, LiveReceiveError, StreamCancellation, StreamError,
    StreamInvocation, live_channel,
};
use std::{sync::mpsc, thread};

fn event(text: &str) -> LiveOutputEvent {
    LiveOutputEvent {
        export: "answer".into(),
        source: vec!["source".into()],
        invocation: StreamInvocation {
            id: 0,
            node: "source".into(),
            path: vec![],
        },
        event: LiveOutputKind::Value { value: text.into() },
    }
}

#[test]
fn record_and_byte_pressure_resume_in_order_without_retaining_history() {
    let value = event(&"x".repeat(32));
    let bytes = serde_json::to_vec(&value).unwrap().len();
    for records in [1, 10] {
        let (publisher, receiver) = live_channel(LiveLimits {
            max_pending_records: records,
            max_pending_bytes: bytes,
            max_record_bytes: bytes,
            max_total_bytes: bytes * 3,
        })
        .unwrap();
        let cancellation = StreamCancellation::default();
        publisher.publish(value.clone(), &cancellation).unwrap();
        let pending = publisher.clone();
        let second = value.clone();
        let (started, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            started.send(()).unwrap();
            pending.publish(second, &cancellation)
        });
        ready.recv().unwrap();
        assert!(!worker.is_finished());
        assert_eq!(receiver.metrics().pending_records, 1);
        assert_eq!(receiver.metrics().pending_bytes, bytes);
        receiver.recv().unwrap().unwrap();
        worker.join().unwrap().unwrap();
        drop(publisher);
        receiver.recv().unwrap().unwrap();
        assert!(receiver.recv().unwrap().is_none());
        assert_eq!(receiver.metrics().pending_bytes, 0);
        assert_eq!(receiver.metrics().total_bytes, bytes * 2);
    }
}

#[test]
fn receiver_drop_wakes_a_blocked_connected_publisher() {
    let (publisher, receiver) = live_channel(LiveLimits {
        max_pending_records: 1,
        ..Default::default()
    })
    .unwrap();
    let cancellation = StreamCancellation::default();
    publisher.publish(event("first"), &cancellation).unwrap();
    let (started, ready) = mpsc::channel();
    let worker = thread::spawn(move || {
        started.send(()).unwrap();
        publisher.publish(event("second"), &cancellation)
    });
    ready.recv().unwrap();
    drop(receiver);
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("receiver disconnected")
    );
}

#[test]
fn cancellation_interrupts_a_full_live_queue() {
    let (publisher, receiver) = live_channel(LiveLimits {
        max_pending_records: 1,
        ..Default::default()
    })
    .unwrap();
    let cancellation = StreamCancellation::default();
    publisher.publish(event("first"), &cancellation).unwrap();
    let cancelled = cancellation.clone();
    let worker = thread::spawn(move || publisher.publish(event("second"), &cancelled));
    cancellation.cancel(StreamError::Execution {
        message: "cancelled".into(),
    });
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(receiver.metrics().pending_records, 1);
}

#[test]
fn per_record_and_total_limits_fail_without_silent_truncation() {
    let (publisher, receiver) = live_channel(LiveLimits {
        max_record_bytes: 128,
        max_pending_bytes: 256,
        max_total_bytes: 512,
        ..Default::default()
    })
    .unwrap();
    let error = publisher
        .publish(event(&"x".repeat(1024)), &StreamCancellation::default())
        .unwrap_err();
    assert!(matches!(error, StreamError::OutputEncode { .. }));
    assert!(matches!(
        receiver.try_recv(),
        Err(LiveReceiveError::Failed { .. })
    ));
    assert_eq!(receiver.metrics().pending_records, 0);

    let value = event("value");
    let bytes = serde_json::to_vec(&value).unwrap().len();
    let (publisher, receiver) = live_channel(LiveLimits {
        max_pending_records: 1,
        max_record_bytes: bytes,
        max_pending_bytes: bytes,
        max_total_bytes: bytes * 2,
    })
    .unwrap();
    for _ in 0..2 {
        publisher
            .publish(value.clone(), &StreamCancellation::default())
            .unwrap();
        receiver.recv().unwrap().unwrap();
    }
    let error = publisher
        .publish(value, &StreamCancellation::default())
        .unwrap_err();
    assert!(matches!(error, StreamError::Resource { .. }));
    assert_eq!(receiver.metrics().total_bytes, bytes * 2);
}

#[test]
fn empty_and_closed_are_distinct_and_invalid_limits_are_rejected() {
    assert!(
        live_channel(LiveLimits {
            max_pending_records: 0,
            ..Default::default()
        })
        .is_err()
    );
    let (publisher, receiver) = live_channel(LiveLimits::default()).unwrap();
    assert!(matches!(receiver.try_recv(), Err(LiveReceiveError::Empty)));
    drop(publisher);
    assert!(matches!(receiver.try_recv(), Err(LiveReceiveError::Closed)));
}

#[test]
fn flush_waits_for_delivery_and_receiver_drop_preserves_failure() {
    for disconnect in [false, true] {
        let (publisher, receiver) = live_channel(LiveLimits::default()).unwrap();
        let cancellation = StreamCancellation::default();
        publisher.publish(event("pending"), &cancellation).unwrap();
        let (started, ready) = mpsc::channel();
        let worker = thread::spawn(move || {
            started.send(()).unwrap();
            publisher.flush(&cancellation)
        });
        ready.recv().unwrap();
        assert!(!worker.is_finished());
        if disconnect {
            drop(receiver);
            assert!(worker.join().unwrap().is_err());
        } else {
            receiver.recv().unwrap().unwrap();
            worker.join().unwrap().unwrap();
            assert!(receiver.recv().unwrap().is_none());
        }
    }
}

#[test]
fn cancellation_remains_observable_after_the_last_publisher_drops() {
    let (publisher, receiver) = live_channel(LiveLimits::default()).unwrap();
    let cancellation = StreamCancellation::default();
    publisher.publish(event("prefix"), &cancellation).unwrap();
    cancellation.cancel(StreamError::Execution {
        message: "cancelled".into(),
    });
    drop(publisher);
    assert!(receiver.recv().unwrap().is_some());
    assert!(
        receiver
            .recv()
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}
