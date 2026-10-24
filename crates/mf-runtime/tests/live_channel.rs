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
fn record_pressure_resumes_in_order_without_retaining_history() {
    let (publisher, receiver) = live_channel(LiveLimits {
        max_pending_records: 1,
    })
    .unwrap();
    let cancellation = StreamCancellation::default();
    publisher.publish(event("first"), &cancellation).unwrap();
    let pending = publisher.clone();
    let (started, ready) = mpsc::channel();
    let worker = thread::spawn(move || {
        started.send(()).unwrap();
        pending.publish(event("second"), &cancellation)
    });
    ready.recv().unwrap();
    assert!(!worker.is_finished());
    assert_eq!(receiver.metrics().pending_records, 1);
    let first = receiver.recv().unwrap().unwrap();
    assert!(
        matches!(first.event, LiveOutputKind::Value { value } if value.as_str() == Some("first"))
    );
    worker.join().unwrap().unwrap();
    drop(publisher);
    let second = receiver.recv().unwrap().unwrap();
    assert!(
        matches!(second.event, LiveOutputKind::Value { value } if value.as_str() == Some("second"))
    );
    assert!(receiver.recv().unwrap().is_none());
    assert_eq!(receiver.metrics().pending_records, 0);
}

#[test]
fn receiver_drop_wakes_a_blocked_connected_publisher() {
    let (publisher, receiver) = live_channel(LiveLimits {
        max_pending_records: 1,
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
fn large_records_and_cumulative_output_have_no_byte_quota() {
    let text = "x".repeat(5 * 1024 * 1024);
    let value = event(&text);
    let (publisher, receiver) = live_channel(LiveLimits {
        max_pending_records: 1,
    })
    .unwrap();
    for _ in 0..13 {
        publisher
            .publish(value.clone(), &StreamCancellation::default())
            .unwrap();
        assert_eq!(receiver.metrics().pending_records, 1);
        let received = receiver.recv().unwrap().unwrap();
        assert!(
            matches!(received.event, LiveOutputKind::Value { value } if value.as_str() == Some(text.as_str()))
        );
    }
    drop(publisher);
    assert!(receiver.recv().unwrap().is_none());
}

#[test]
fn empty_and_closed_are_distinct_and_invalid_limits_are_rejected() {
    assert!(
        live_channel(LiveLimits {
            max_pending_records: 0,
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
