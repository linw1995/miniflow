use mf_runtime::{EventRecorder, ExecutionContext};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn event_recorders_follow_stream_domain_and_iteration_contexts() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let observed = records.clone();
    let mut context = ExecutionContext::default();
    context.set_event_recorder(EventRecorder::new(move |event| {
        observed.lock().unwrap().push(event.clone())
    }));
    let children = [
        context.fork_stream(None),
        context.fork_domain_with_observation(None),
        context.fork_body(None),
    ];
    std::thread::scope(|scope| {
        for child in children {
            scope.spawn(move || {
                let timestamp = child
                    .record_event("fixture.event", json!({"value":1}))
                    .unwrap();
                assert!(timestamp > 0);
            });
        }
    });
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 3);
    assert!(
        records
            .iter()
            .all(|event| event.name == "fixture.event" && event.body["value"] == 1)
    );
}

#[test]
fn scoped_recorders_restore_after_success_and_unwind_and_observer_panics_are_isolated() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = seen.clone();
    let mut context = ExecutionContext::default();
    context.set_event_recorder(EventRecorder::new(move |event| {
        observed.lock().unwrap().push(event.name.clone())
    }));
    context.with_event_recorder(
        EventRecorder::new(|_| panic!("observer failure")),
        |child| {
            assert!(child.record_event("fixture.ignored", json!({})).is_some());
        },
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        context.with_event_recorder(EventRecorder::new(|_| {}), |_| panic!("execution failure"));
    }));
    assert!(result.is_err());
    context.record_event("fixture.restored", json!({}));
    assert_eq!(*seen.lock().unwrap(), ["fixture.restored"]);
}
