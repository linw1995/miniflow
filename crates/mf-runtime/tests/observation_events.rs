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
