//! Self-contained snapshot observation records and a stateless publication session.
use crate::event::LoopPathEntry;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu, ensure};
use std::sync::{Arc, Mutex};

pub const SNAPSHOT_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotOutcome {
    Started,
    Succeeded,
    Failed,
    Skipped,
    Emitted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSnapshot<V> {
    pub inputs: V,
    pub outputs: V,
    pub skipped: Arc<[String]>,
    pub outcome: SnapshotOutcome,
    pub error: Option<Arc<str>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotEvent<V> {
    pub scope: Vec<LoopPathEntry>,
    pub node: Arc<str>,
    pub snapshot: NodeSnapshot<V>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotRecord<V> {
    Header {
        version: u32,
    },
    Node {
        scope: Vec<LoopPathEntry>,
        node: Arc<str>,
        inputs: V,
        outputs: V,
        skipped: Arc<[String]>,
        outcome: SnapshotOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<Arc<str>>,
    },
    End,
}

impl<V> From<SnapshotEvent<V>> for SnapshotRecord<V> {
    fn from(event: SnapshotEvent<V>) -> Self {
        let SnapshotEvent {
            scope,
            node,
            snapshot,
        } = event;
        let NodeSnapshot {
            inputs,
            outputs,
            skipped,
            outcome,
            error,
        } = snapshot;
        Self::Node {
            scope,
            node,
            inputs,
            outputs,
            skipped,
            outcome,
            error,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum SnapshotPublishError {
    #[snafu(display("invalid snapshot: {message}"))]
    Invalid { message: &'static str },
    #[snafu(display("could not encode snapshot: {source}"))]
    Encode { source: serde_json::Error },
    #[snafu(display("snapshot sink failed: {source}"))]
    Sink {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

type SnapshotSink = Box<
    dyn FnMut(serde_json::Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> + Send,
>;

struct Publication {
    sink: SnapshotSink,
    error: Option<Arc<SnapshotPublishError>>,
    complete: bool,
}

/// Owns only sink lifecycle and diagnostics. Payloads are released after each call.
#[derive(Clone)]
pub struct SnapshotPublisher(Arc<Mutex<Publication>>);

impl SnapshotPublisher {
    pub fn new(
        mut sink: impl FnMut(serde_json::Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + 'static,
    ) -> Result<Self, SnapshotPublishError> {
        let header = serde_json::to_value(SnapshotRecord::<serde_json::Value>::Header {
            version: SNAPSHOT_VERSION,
        })
        .context(EncodeSnafu)?;
        sink(header).context(SinkSnafu)?;
        Ok(Self(Arc::new(Mutex::new(Publication {
            sink: Box::new(sink),
            error: None,
            complete: false,
        }))))
    }

    /// Publishes an event already projected for observation. Runtime callers must
    /// connect this to `SnapshotRecorder`, which removes value-specific secret data.
    pub fn record<V: Serialize>(&self, event: SnapshotEvent<V>) {
        let mut state = self.0.lock().expect("snapshot publisher was not poisoned");
        if state.complete || state.error.is_some() {
            return;
        }
        let result = (|| {
            let record = serde_json::to_value(SnapshotRecord::from(event)).context(EncodeSnafu)?;
            ensure!(
                record["inputs"].is_object() && record["outputs"].is_object(),
                InvalidSnafu {
                    message: "node snapshots require input and output objects"
                }
            );
            (state.sink)(record).context(SinkSnafu)
        })();
        if let Err(error) = result {
            state.error = Some(Arc::new(error));
        }
    }

    pub fn finish(&self) {
        let mut state = self.0.lock().expect("snapshot publisher was not poisoned");
        if state.complete || state.error.is_some() {
            return;
        }
        let record = serde_json::json!({"record": "end"});
        match (state.sink)(record).context(SinkSnafu) {
            Ok(()) => state.complete = true,
            Err(error) => state.error = Some(Arc::new(error)),
        }
    }

    pub fn diagnostic(&self) -> Option<Arc<SnapshotPublishError>> {
        self.0
            .lock()
            .expect("snapshot publisher was not poisoned")
            .error
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        error::Error,
        io,
        sync::atomic::{AtomicUsize, Ordering},
    };

    fn event(value: Value) -> SnapshotEvent<Arc<Value>> {
        SnapshotEvent {
            scope: vec![],
            node: "node".into(),
            snapshot: NodeSnapshot {
                inputs: Arc::new(json!({})),
                outputs: Arc::new(value),
                skipped: Arc::from([]),
                outcome: SnapshotOutcome::Emitted,
                error: None,
            },
        }
    }

    #[test]
    fn publication_releases_every_payload_and_preserves_repeated_boundaries() {
        let count = Arc::new(AtomicUsize::new(0));
        let captured = count.clone();
        let publisher = SnapshotPublisher::new(move |record| {
            if record["record"] == "node" {
                assert_eq!(record["outputs"]["value"], json!(42));
            }
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        for _ in 0..1000 {
            let event = event(json!({"value": 42}));
            let weak = Arc::downgrade(&event.snapshot.outputs);
            publisher.record(event);
            assert!(weak.upgrade().is_none());
        }
        publisher.finish();
        publisher.finish();
        publisher.record(event(json!({"value": 99})));
        assert_eq!(count.load(Ordering::SeqCst), 1002);
        assert!(publisher.diagnostic().is_none());
    }

    #[test]
    fn invalid_port_objects_are_rejected_before_publication() {
        for inputs_invalid in [false, true] {
            let count = Arc::new(AtomicUsize::new(0));
            let captured = count.clone();
            let publisher = SnapshotPublisher::new(move |_| {
                captured.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
            let mut invalid = event(json!({}));
            if inputs_invalid {
                invalid.snapshot.inputs = Arc::new(Value::Null);
            } else {
                invalid.snapshot.outputs = Arc::new(Value::Null);
            }
            publisher.record(invalid);
            publisher.record(event(json!({})));
            publisher.finish();
            assert!(matches!(
                publisher.diagnostic().as_deref(),
                Some(SnapshotPublishError::Invalid { .. })
            ));
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn sink_failures_preserve_their_source_and_prevent_a_false_end_marker() {
        for fail_on in ["header", "node", "end"] {
            let publisher = SnapshotPublisher::new(move |record| {
                if record["record"] == fail_on {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed").into());
                }
                Ok(())
            });
            let error = match publisher {
                Err(error) => Arc::new(error),
                Ok(publisher) => {
                    publisher.record(event(json!({})));
                    publisher.finish();
                    publisher.diagnostic().unwrap()
                }
            };
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .kind(),
                io::ErrorKind::BrokenPipe
            );
        }
    }
}
