//! Shared business value history received through OTLP logs.
use crate::traces::SpanKey;
use mf_runtime::{NodeSnapshot, SnapshotOutcome, ValueRef};
use mf_telemetry::snapshot::Assembler;
use mf_telemetry::{capture::SNAPSHOT_VERSION, event::LoopPathEntry};
use opentelemetry_proto::tonic::{common::v1::any_value, logs::v1::LogRecord};
use rpds::RedBlackTreeMapSync;
use snafu::{ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Arc,
};

pub type SnapshotRecord = mf_telemetry::capture::SnapshotRecord<ValueRef>;

#[derive(Debug, Snafu)]
pub enum CaptureError {
    #[snafu(transparent)]
    Transport {
        source: mf_telemetry::snapshot::SnapshotError,
    },
    #[snafu(display("invalid snapshot record: {source}"))]
    Record { source: serde_json::Error },
    #[snafu(transparent)]
    Store { source: SnapshotStoreError },
    #[snafu(display("snapshot sequence {sequence} has conflicting trace context"))]
    ConflictingContext { sequence: usize },
    #[snafu(display("snapshot capture is closed"))]
    Closed,
    #[snafu(display("{message}"))]
    Unavailable { message: String },
    #[snafu(display(
        "snapshot history is incomplete at sequence {sequence}; received history remains available"
    ))]
    Incomplete { sequence: usize },
}

#[derive(Debug, Snafu)]
pub enum SnapshotStoreError {
    #[snafu(display("{message}"))]
    Invalid { message: &'static str },
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    scopes:
        RedBlackTreeMapSync<Vec<LoopPathEntry>, RedBlackTreeMapSync<Arc<str>, Arc<NodeSnapshot>>>,
}

impl Snapshot {
    pub fn node(&self, scope: &[LoopPathEntry], node: &str) -> Option<&Arc<NodeSnapshot>> {
        self.scopes.get(scope)?.get(node)
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.scopes.ptr_eq(&other.scopes)
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotEntry {
    pub scope: Vec<LoopPathEntry>,
    pub node: Arc<str>,
    pub snapshot: Snapshot,
}

/// Receiver-owned immutable history. No value dictionary is required for replay.
#[derive(Debug, Default)]
pub struct SnapshotStore {
    current: Snapshot,
    history: Vec<SnapshotEntry>,
    initialized: bool,
    complete: bool,
}

impl SnapshotStore {
    pub fn current(&self) -> &Snapshot {
        &self.current
    }
    pub fn history(&self) -> &[SnapshotEntry] {
        &self.history
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn apply(&mut self, record: SnapshotRecord) -> Result<bool, SnapshotStoreError> {
        ensure!(
            !self.complete,
            InvalidSnafu {
                message: "snapshot record followed the end marker"
            }
        );
        if let SnapshotRecord::Header { version } = record {
            ensure!(
                !self.initialized && version == SNAPSHOT_VERSION,
                InvalidSnafu {
                    message: "unsupported or repeated snapshot header; recompile the runner for this receiver"
                }
            );
            self.initialized = true;
            return Ok(false);
        }
        ensure!(
            self.initialized,
            InvalidSnafu {
                message: "snapshot stream omitted its header"
            }
        );
        match record {
            SnapshotRecord::Node {
                scope,
                node,
                inputs,
                outputs,
                skipped,
                outcome,
                error,
            } => {
                ensure!(
                    inputs.is_object() && outputs.is_object(),
                    InvalidSnafu {
                        message: "node snapshots require input and output objects"
                    }
                );
                let next = NodeSnapshot {
                    inputs,
                    outputs,
                    skipped,
                    outcome,
                    error,
                };
                let previous = self.current.node(&scope, &node);
                let equal = previous.is_some_and(|previous| previous.as_ref() == &next);
                if equal && !matches!(outcome, SnapshotOutcome::Started | SnapshotOutcome::Emitted)
                {
                    return Ok(false);
                }
                // Repeated invocation boundaries remain visible, while an unchanged node
                // can share its immutable payload with its previous occurrence.
                let next = if equal {
                    previous.unwrap().clone()
                } else {
                    Arc::new(next)
                };
                let mut nodes = self.current.scopes.get(&scope).cloned().unwrap_or_default();
                nodes.insert_mut(node.clone(), next);
                self.current.scopes.insert_mut(scope.clone(), nodes);
                self.history.push(SnapshotEntry {
                    scope,
                    node,
                    snapshot: self.current.clone(),
                });
                Ok(true)
            }
            SnapshotRecord::End => {
                self.complete = true;
                Ok(false)
            }
            SnapshotRecord::Header { .. } => unreachable!("headers were handled above"),
        }
    }
}

const PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct SnapshotCapture {
    pub store: SnapshotStore,
    pub diagnostic: Option<Arc<CaptureError>>,
    assembler: Assembler,
    finished: bool,
    pending_contexts: BTreeMap<usize, SnapshotMetadata>,
    metadata: Vec<SnapshotMetadata>,
    cached: Option<Arc<HistorySnapshot>>,
    nodes_by_span: BTreeMap<SpanKey, Arc<NodeSnapshot>>,
}

#[derive(Clone, Debug, Default)]
pub struct SpanData {
    pub node: Option<Arc<NodeSnapshot>>,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct SnapshotMetadata {
    pub sequence: usize,
    pub time_unix_nano: u64,
    /// Local admission time, independent of exporter batching and wall-clock adjustments.
    pub received_at: Option<std::time::Instant>,
    pub correlation: Option<SpanKey>,
}

#[derive(Clone, Debug, Default)]
pub struct HistorySnapshot {
    pub history_len: usize,
    pub selected: usize,
    pub first: usize,
    pub entries: Vec<SnapshotEntry>,
    pub metadata: Vec<SnapshotMetadata>,
    pub status: String,
}

impl SnapshotCapture {
    pub fn admit(&mut self, record: LogRecord) -> Result<(), Arc<CaptureError>> {
        if self.finished {
            return Err(Arc::new(ClosedSnafu.build()));
        }
        if let Some(error) = &self.diagnostic {
            return Err(error.clone());
        }
        self.cached = None;
        let result = (|| -> Result<(), CaptureError> {
            let first = self.assembler.next_sequence();
            let sequence = record
                .attributes
                .iter()
                .find(|attribute| attribute.key == "mf.snapshot.sequence")
                .and_then(|attribute| attribute.value.as_ref())
                .and_then(|value| value.value.as_ref())
                .and_then(|value| match value {
                    any_value::Value::IntValue(sequence) => usize::try_from(*sequence).ok(),
                    _ => None,
                });
            let correlation = (record.trace_id.len() == 16
                && record.span_id.len() == 8
                && record.trace_id.iter().any(|byte| *byte != 0)
                && record.span_id.iter().any(|byte| *byte != 0))
            .then(|| (record.trace_id.clone(), record.span_id.clone()));
            if let Some(sequence) = sequence.filter(|sequence| *sequence >= first) {
                ensure!(
                    self.pending_contexts
                        .get(&sequence)
                        .is_none_or(|previous| previous.correlation == correlation),
                    ConflictingContextSnafu { sequence }
                );
                self.pending_contexts
                    .entry(sequence)
                    .or_insert(SnapshotMetadata {
                        sequence,
                        time_unix_nano: record.time_unix_nano,
                        received_at: None,
                        correlation,
                    });
            }
            for (offset, body) in self.assembler.push(record)?.into_iter().enumerate() {
                let record: SnapshotRecord = serde_json::from_value(body).context(RecordSnafu)?;
                let node = match &record {
                    SnapshotRecord::Node { scope, node, .. } => Some((scope.clone(), node.clone())),
                    _ => None,
                };
                let changed = self.store.apply(record)?;
                let mut metadata = self
                    .pending_contexts
                    .remove(&(first + offset))
                    .expect("assembled records retain their packet metadata");
                let correlation = metadata.correlation.clone();
                if changed {
                    metadata.received_at = Some(std::time::Instant::now());
                    self.metadata.push(metadata);
                }
                if let (Some((scope, node)), Some(correlation)) = (node, correlation) {
                    let snapshot = self
                        .store
                        .current()
                        .node(&scope, &node)
                        .expect("admitted node snapshots exist in the store")
                        .clone();
                    self.nodes_by_span.insert(correlation, snapshot);
                }
            }
            Ok(())
        })()
        .map_err(Arc::new);
        if let Err(error) = &result {
            self.diagnostic = Some(error.clone());
        }
        result
    }

    pub fn fail(&mut self, error: &str) {
        self.cached = None;
        self.diagnostic
            .get_or_insert_with(|| Arc::new(UnavailableSnafu { message: error }.build()));
    }

    pub fn finish(&mut self) {
        self.cached = None;
        self.finished = true;
        if self.diagnostic.is_none()
            && (self.assembler.has_pending()
                || (self.store.is_initialized() && !self.store.is_complete()))
        {
            self.diagnostic = Some(Arc::new(
                IncompleteSnafu {
                    sequence: self.assembler.next_sequence(),
                }
                .build(),
            ));
        }
    }

    pub fn data(&self, trace_id: &[u8], span_id: &[u8]) -> SpanData {
        SpanData {
            node: self
                .nodes_by_span
                .get(&(trace_id.to_vec(), span_id.to_vec()))
                .cloned(),
            status: self.status(),
        }
    }

    fn status(&self) -> String {
        if let Some(error) = &self.diagnostic {
            error.to_string()
        } else if self.store.is_complete() {
            "Complete".into()
        } else if self.finished && !self.store.is_initialized() {
            "Data snapshots unavailable; the runner must support telemetry and snapshot capture"
                .into()
        } else {
            "Collecting".into()
        }
    }

    pub fn snapshot(&mut self) -> Arc<HistorySnapshot> {
        if let Some(snapshot) = &self.cached {
            return snapshot.clone();
        }
        let snapshot = Arc::new(self.view(None, usize::MAX));
        self.cached = Some(snapshot.clone());
        snapshot
    }

    /// Copy only the live animation window, not the complete session history.
    pub fn recent(&self, window: std::time::Duration) -> HistorySnapshot {
        let now = std::time::Instant::now();
        let first = self.metadata.partition_point(|metadata| {
            metadata
                .received_at
                .is_some_and(|received_at| now.saturating_duration_since(received_at) > window)
        });
        let history = self.store.history();
        HistorySnapshot {
            history_len: history.len(),
            selected: history.len().saturating_sub(1),
            first,
            entries: history[first..].to_vec(),
            metadata: self.metadata[first..].to_vec(),
            status: self.status(),
        }
    }

    pub fn view(&self, selected: Option<usize>, rows: usize) -> HistorySnapshot {
        let history = self.store.history();
        let selected = selected
            .unwrap_or(history.len().saturating_sub(1))
            .min(history.len().saturating_sub(1));
        let rows = rows.max(1);
        let first = selected
            .saturating_sub(rows / 2)
            .min(history.len().saturating_sub(rows));
        let status = self.status();
        HistorySnapshot {
            history_len: history.len(),
            selected,
            first,
            entries: history.iter().skip(first).take(rows).cloned().collect(),
            metadata: self
                .metadata
                .iter()
                .skip(first)
                .take(rows)
                .cloned()
                .collect(),
            status,
        }
    }
}

pub fn span_data_preview(data: &SpanData, finished: bool) -> String {
    let mut text = format!("Data capture: {}\n", data.status);
    if let Some(node) = &data.node {
        text.push_str(&format!("Outcome: {:?}\n", node.outcome));
        if let Some(error) = &node.error {
            let message: String = error.chars().take(4096).collect();
            text.push_str(&format!("Error: {message:?}\n"));
        }
        if !node.skipped.is_empty() {
            text.push_str(&format!("Skipped outputs: {:?}\n", node.skipped));
        }
        text.push_str(&format!(
            "\nInputs:\n{}\n\nOutputs:\n{}",
            value_preview(&node.inputs),
            value_preview(&node.outputs)
        ));
    } else if finished {
        text.push_str("No node snapshots are associated with this selection.\nSelect a node span or node-correlated log. Records without valid trace/span context cannot be matched.");
    } else {
        text.push_str("Waiting for matching node snapshots.\nUse g to inspect live session logs while spans are still running.");
    }
    text
}

fn value_preview(value: &ValueRef) -> String {
    struct Preview(Vec<u8>);
    impl Write for Preview {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let remaining = PREVIEW_BYTES.saturating_sub(self.0.len());
            if remaining == 0 {
                return Err(io::Error::other("preview limit"));
            }
            let count = remaining.min(bytes.len());
            self.0.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut preview = Preview(Vec::new());
    let truncated = serde_json::to_writer_pretty(&mut preview, value).is_err();
    let mut text = String::from_utf8_lossy(&preview.0).into_owned();
    if truncated {
        text.push_str("\n[Preview truncated at 64 KiB; the complete value remains in history.]");
    }
    text
}

#[cfg(test)]
pub struct TestHistory(std::sync::Mutex<SnapshotStore>);
#[cfg(test)]
impl TestHistory {
    pub fn memory() -> Self {
        let mut store = SnapshotStore::default();
        store
            .apply(SnapshotRecord::Header {
                version: SNAPSHOT_VERSION,
            })
            .unwrap();
        Self(std::sync::Mutex::new(store))
    }
    pub fn record(&self, scope: Vec<LoopPathEntry>, node: &str, snapshot: NodeSnapshot) {
        self.0
            .lock()
            .unwrap()
            .apply(
                mf_runtime::SnapshotEvent {
                    scope,
                    node: node.into(),
                    snapshot,
                }
                .into(),
            )
            .unwrap();
    }
    pub fn history(&self) -> Vec<SnapshotEntry> {
        self.0.lock().unwrap().history().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{NodeSnapshot, SnapshotOutcome};
    use mf_telemetry::capture::SnapshotPublisher;
    use mf_telemetry::snapshot::packets;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    struct TestPublisher(SnapshotPublisher);
    impl TestPublisher {
        fn record(&self, scope: Vec<LoopPathEntry>, node: &str, snapshot: NodeSnapshot) {
            let publisher = self.0.clone();
            mf_runtime::SnapshotRecorder::new(move |event| publisher.record(event)).record(
                mf_runtime::SnapshotEvent {
                    scope,
                    node: node.into(),
                    snapshot,
                },
            );
        }
        fn finish(&self) {
            self.0.finish();
        }
    }

    fn snapshot(value: ValueRef) -> NodeSnapshot {
        NodeSnapshot {
            inputs: ValueRef::object([]),
            outputs: ValueRef::object([("value".into(), value)]),
            skipped: Arc::from([]),
            outcome: SnapshotOutcome::Succeeded,
            error: None,
        }
    }
    fn recorder(capture: Arc<Mutex<SnapshotCapture>>) -> TestPublisher {
        let mut sequence = 0;
        TestPublisher(
            SnapshotPublisher::new(move |record| {
                for packet in packets(sequence, record)? {
                    capture
                        .lock()
                        .unwrap()
                        .admit(packet.into_log_record("workflow", "run"))?;
                }
                sequence += 1;
                Ok(())
            })
            .unwrap(),
        )
    }

    #[test]
    fn animation_window_copies_only_recent_records_and_keeps_history_intact() {
        let capture = Arc::new(Mutex::new(SnapshotCapture::default()));
        let publisher = recorder(capture.clone());
        for value in 0..3 {
            publisher.record(Vec::new(), "source", snapshot(ValueRef::from(value)));
        }
        let mut capture = capture.lock().unwrap();
        let now = std::time::Instant::now();
        capture.metadata[0].received_at = Some(now - std::time::Duration::from_secs(10));
        capture.metadata[1].received_at = Some(now - std::time::Duration::from_secs(5));
        capture.metadata[2].received_at = Some(now);
        let recent = capture.recent(std::time::Duration::from_secs(1));
        assert_eq!(recent.first, 2);
        assert_eq!(recent.history_len, 3);
        assert_eq!(recent.entries.len(), 1);
        assert_eq!(recent.metadata.len(), 1);
        assert_eq!(capture.store.history().len(), 3);
        capture.metadata[2].received_at = Some(now - std::time::Duration::from_secs(2));
        assert!(
            capture
                .recent(std::time::Duration::from_secs(1))
                .entries
                .is_empty()
        );
    }

    #[test]
    fn receiver_validates_session_order_and_versions_before_changing_history() {
        let mut store = SnapshotStore::default();
        assert!(store.apply(SnapshotRecord::End).is_err());
        assert!(store.apply(SnapshotRecord::Header { version: 1 }).is_err());
        store
            .apply(SnapshotRecord::Header {
                version: SNAPSHOT_VERSION,
            })
            .unwrap();
        assert!(
            store
                .apply(SnapshotRecord::Header {
                    version: SNAPSHOT_VERSION
                })
                .is_err()
        );
        let event = mf_runtime::SnapshotEvent {
            scope: vec![],
            node: "node".into(),
            snapshot: snapshot(42.into()),
        };
        assert!(store.apply(event.clone().into()).unwrap());
        let first = store.current().clone();
        assert!(!store.apply(event.into()).unwrap());
        assert!(store.current().ptr_eq(&first));
        assert_eq!(store.history().len(), 1);
        store.apply(SnapshotRecord::End).unwrap();
        assert!(store.apply(SnapshotRecord::End).is_err());
        assert!(store.is_complete());
    }

    #[test]
    fn capture_retains_record_decode_sources_until_presentation() {
        use std::error::Error;
        let mut capture = SnapshotCapture::default();
        let record = packets(0, json!({"record":"unknown"}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        let error = capture.admit(record.clone()).unwrap_err();
        assert!(error.source().unwrap().is::<serde_json::Error>());
        let repeated = capture.admit(record).unwrap_err();
        assert!(repeated.source().unwrap().is::<serde_json::Error>());
        capture.finish();
        assert_eq!(capture.view(None, 1).status, error.to_string());
    }

    #[test]
    fn otlp_history_preserves_historical_roots_and_unchanged_nodes() {
        let capture = Arc::new(Mutex::new(SnapshotCapture::default()));
        let recorder = recorder(capture.clone());
        let value = ValueRef::from(json!({"payload": "unique payload", "count": 1}));
        recorder.record(vec![], "first", snapshot(value.clone()));
        let first = capture.lock().unwrap().store.current().clone();
        recorder.record(
            vec![],
            "second",
            snapshot(ValueRef::from(serde_json::to_value(&value).unwrap())),
        );
        recorder.record(
            vec![],
            "first",
            snapshot(json!({"payload": "unique payload", "count": 2}).into()),
        );
        let started = NodeSnapshot {
            inputs: json!({"item": 1}).into(),
            outputs: ValueRef::object([]),
            skipped: Arc::from([]),
            outcome: SnapshotOutcome::Started,
            error: None,
        };
        recorder.record(vec![], "buffer", started.clone());
        recorder.record(vec![], "buffer", started);
        recorder.finish();
        let mut capture = capture.lock().unwrap();
        capture.finish();
        assert!(capture.diagnostic.is_none());
        assert!(capture.store.is_complete());
        assert_eq!(capture.store.history().len(), 5);
        assert_eq!(
            first.node(&[], "first").unwrap().outputs["value"]["count"],
            json!(1)
        );
        let latest = capture.store.current();
        assert_eq!(
            first.node(&[], "first").unwrap().outputs["value"]["count"],
            json!(1)
        );
        assert_eq!(
            latest.node(&[], "first").unwrap().outputs["value"]["count"],
            json!(2)
        );
        assert!(Arc::ptr_eq(
            capture.store.history()[1]
                .snapshot
                .node(&[], "second")
                .unwrap(),
            latest.node(&[], "second").unwrap(),
        ));
        let window = capture.view(None, 1);
        assert_eq!(window.entries.len(), 1);
        assert_eq!(window.history_len, 5);
        assert!(window.entries[0].snapshot.ptr_eq(latest));
    }

    #[test]
    fn missing_records_and_invalid_node_payloads_leave_explicit_incomplete_history() {
        let mut capture = SnapshotCapture::default();
        let header = serde_json::to_value(SnapshotRecord::Header {
            version: SNAPSHOT_VERSION,
        })
        .unwrap();
        for packet in packets(0, header).unwrap() {
            capture
                .admit(packet.into_log_record("workflow", "run"))
                .unwrap();
        }
        let end = serde_json::to_value(SnapshotRecord::End).unwrap();
        for packet in packets(2, end).unwrap() {
            capture
                .admit(packet.into_log_record("workflow", "run"))
                .unwrap();
        }
        capture.finish();
        assert!(
            capture
                .diagnostic
                .as_ref()
                .unwrap()
                .to_string()
                .contains("sequence 1")
        );
        assert!(!capture.store.is_complete());

        let mut capture = SnapshotCapture::default();
        for (sequence, record) in [
            SnapshotRecord::Header {
                version: SNAPSHOT_VERSION,
            },
            SnapshotRecord::Node {
                scope: vec![],
                node: "invalid".into(),
                inputs: ValueRef::null(),
                outputs: ValueRef::object([]),
                skipped: Arc::from([]),
                outcome: SnapshotOutcome::Succeeded,
                error: None,
            },
        ]
        .into_iter()
        .enumerate()
        {
            for packet in packets(sequence, serde_json::to_value(record).unwrap()).unwrap() {
                let _ = capture.admit(packet.into_log_record("workflow", "run"));
            }
        }
        assert!(
            capture
                .diagnostic
                .as_ref()
                .unwrap()
                .to_string()
                .contains("node snapshots require input and output objects")
        );
        assert!(capture.store.history().is_empty());
    }

    #[test]
    fn large_previews_are_bounded_without_truncating_stored_values() {
        let value = ValueRef::from("x".repeat(PREVIEW_BYTES * 2));
        let preview = value_preview(&value);
        assert!(preview.contains("Preview truncated"));
        assert_eq!(value.as_str().unwrap().len(), PREVIEW_BYTES * 2);
    }
    #[test]
    fn reordered_records_keep_repeated_nodes_associated_with_their_own_spans() {
        let mut capture = SnapshotCapture::default();
        let bodies = [
            json!({"record":"header","version":SNAPSHOT_VERSION}),
            json!({"record":"node","scope":[],"node":"same","inputs":{},"outputs":{"result":"first invocation"},"skipped":[],"outcome":"succeeded"}),
            json!({"record":"node","scope":[],"node":"same","inputs":{},"outputs":{"result":"second invocation"},"skipped":[],"outcome":"succeeded"}),
            json!({"record":"end"}),
        ];
        for (sequence, body) in bodies.into_iter().enumerate().rev() {
            for packet in packets(sequence, body).unwrap() {
                let mut record = packet.into_log_record("workflow", "run");
                record.trace_id = vec![1; 16];
                record.span_id = vec![if sequence < 2 { 1 } else { 2 }; 8];
                capture.admit(record).unwrap();
            }
        }
        capture.finish();
        assert_eq!(
            capture.data(&[1; 16], &[1; 8]).node.unwrap().outputs["result"],
            json!("first invocation")
        );
        assert_eq!(
            capture.data(&[1; 16], &[2; 8]).node.unwrap().outputs["result"],
            json!("second invocation")
        );
        assert!(capture.data(&[2; 16], &[1; 8]).node.is_none());
        let history = capture.snapshot();
        assert_eq!(history.entries.len(), 2);
        assert_eq!(
            history
                .metadata
                .iter()
                .map(|metadata| metadata.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            history.metadata[0].correlation,
            Some((vec![1; 16], vec![1; 8]))
        );
        assert_eq!(
            history.metadata[1].correlation,
            Some((vec![1; 16], vec![2; 8]))
        );
        assert!(Arc::ptr_eq(&history, &capture.snapshot()));
        assert!(capture.pending_contexts.is_empty());
        assert_eq!(capture.data(&[1; 16], &[1; 8]).status, "Complete");
    }

    #[test]
    fn fragmented_node_records_preserve_context_and_reject_conflicting_fragments() {
        let mut capture = SnapshotCapture::default();
        for (sequence, body) in [json!({"record":"header","version":SNAPSHOT_VERSION})]
            .into_iter()
            .enumerate()
        {
            for packet in packets(sequence, body).unwrap() {
                capture
                    .admit(packet.into_log_record("workflow", "run"))
                    .unwrap();
            }
        }
        let body = json!({"record":"node","scope":[],"node":"step","inputs":{},"outputs":{},"skipped":[],"outcome":"failed","error":"x".repeat(100_000)});
        let mut records: Vec<_> = packets(1, body.clone())
            .unwrap()
            .map(|packet| {
                let mut record = packet.into_log_record("workflow", "run");
                record.trace_id = vec![1; 16];
                record.span_id = vec![2; 8];
                record
            })
            .collect();
        assert!(records.len() > 1);
        for record in records.iter().rev() {
            capture.admit(record.clone()).unwrap();
        }
        assert_eq!(
            capture
                .data(&[1; 16], &[2; 8])
                .node
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .len(),
            100_000
        );
        assert!(capture.pending_contexts.is_empty());
        let mut capture = SnapshotCapture::default();
        capture.admit(records.remove(0)).unwrap();
        records[0].span_id = vec![3; 8];
        let error = capture.admit(records[0].clone()).unwrap_err();
        assert!(matches!(
            error.as_ref(),
            CaptureError::ConflictingContext { sequence: 1 }
        ));
        assert!(matches!(
            capture.admit(records[0].clone()).unwrap_err().as_ref(),
            CaptureError::ConflictingContext { sequence: 1 }
        ));
    }

    #[test]
    fn missing_context_does_not_guess_a_span_and_retries_do_not_grow_pending_contexts() {
        let capture = Arc::new(Mutex::new(SnapshotCapture::default()));
        let recorder = recorder(capture.clone());
        recorder.record(vec![], "step", snapshot(42.into()));
        recorder.finish();
        let mut capture = capture.lock().unwrap();
        capture.finish();
        assert!(capture.data(&[1; 16], &[2; 8]).node.is_none());
        let duplicate = packets(0, json!({"record":"header","version":SNAPSHOT_VERSION}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        let mut capture = SnapshotCapture::default();
        for _ in 0..100 {
            capture.admit(duplicate.clone()).unwrap();
        }
        assert!(capture.pending_contexts.is_empty());
    }
}
