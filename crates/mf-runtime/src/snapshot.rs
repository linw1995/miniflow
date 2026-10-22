use crate::ValueRef;
use std::sync::Arc;

pub use mf_telemetry::capture::SnapshotOutcome;
pub type NodeSnapshot = mf_telemetry::capture::NodeSnapshot<ValueRef>;
pub type SnapshotEvent = mf_telemetry::capture::SnapshotEvent<ValueRef>;

type SnapshotCallback = dyn Fn(SnapshotEvent) + Send + Sync;

/// A notification handle with no encoding, dictionary, or history retention.
#[derive(Clone)]
pub struct SnapshotRecorder {
    callback: Arc<SnapshotCallback>,
}

impl std::fmt::Debug for SnapshotRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SnapshotRecorder")
            .finish_non_exhaustive()
    }
}

impl SnapshotRecorder {
    /// The callback must not unwind. Capture backpressure belongs to the observer.
    pub fn new(callback: impl Fn(SnapshotEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    pub fn record(&self, mut event: SnapshotEvent) {
        event.snapshot.inputs = event.snapshot.inputs.redacted();
        event.snapshot.outputs = event.snapshot.outputs.redacted();
        (self.callback)(event);
    }
}

pub fn ports_snapshot(ports: &crate::Inputs) -> ValueRef {
    ValueRef::object(
        ports
            .iter()
            .map(|(name, value)| (Arc::from(name.as_str()), value.clone())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn callbacks_receive_redacted_values_without_retaining_payloads() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let recorder = SnapshotRecorder::new(move |event| captured.lock().unwrap().push(event));
        let public = ValueRef::from("visible");
        let secret = ValueRef::from("credential").into_secret();
        recorder.record(SnapshotEvent {
            scope: vec![],
            node: "node".into(),
            snapshot: NodeSnapshot {
                inputs: ports_snapshot(&crate::Inputs::from([("secret".into(), secret)])),
                outputs: ports_snapshot(&crate::Outputs::from([("public".into(), public.clone())])),
                skipped: Arc::from([]),
                outcome: SnapshotOutcome::Emitted,
                error: None,
            },
        });
        let event = seen.lock().unwrap().pop().unwrap();
        assert_eq!(event.snapshot.inputs["secret"].as_str(), Some("[REDACTED]"));
        assert!(event.snapshot.outputs["public"].ptr_eq(&public));
        assert!(
            !serde_json::to_string(&event)
                .unwrap()
                .contains("credential")
        );
    }
}
