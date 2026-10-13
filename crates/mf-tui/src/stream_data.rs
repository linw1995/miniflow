//! Bounded stream data retention independent of lifecycle and raw-log budgets.

use crate::traces::SpanKey;
use mf_telemetry::{
    identity::{RunId, WorkflowId},
    stream_data::{self, CaptureCounts, DataError, DataPayload, DataRecord, DataValue},
};
use sha2::{Digest, Sha256};
use snafu::{Snafu, ensure};
use std::{collections::BTreeMap, sync::Arc};

pub const MAX_DATA_RECORDS: usize = 4096;
pub const MAX_DATA_BYTES: usize = 16 * 1024 * 1024;
const MAX_WITNESSES: usize = 4096;
const WITNESS_BYTES: usize = 128;
// Reserve space for the first typed decoder error, including its bounded input.
const DIAGNOSTIC_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub enum DataStoreError {
    #[snafu(transparent)]
    Decode { source: DataError },
    #[snafu(display("conflicting stream data record #{sequence}"))]
    Conflict { sequence: u64 },
    #[snafu(display("invalid stream data history: {message}"))]
    History { message: &'static str },
}

#[derive(Clone, Debug)]
pub struct DataObservation {
    pub record: DataRecord,
    pub correlation: Option<SpanKey>,
}

#[derive(Clone, Debug, Default)]
pub struct DataSnapshot {
    pub streaming: bool,
    pub records: Vec<Arc<DataObservation>>,
    pub status: String,
    pub observed: u64,
    pub evicted: u64,
    pub old_retransmissions: u64,
    pub omissions: u64,
    pub sender_drops: u64,
    pub gaps: Option<u64>,
    pub faults: u64,
    pub diagnostic: Option<Arc<DataStoreError>>,
    pub retained_bytes: usize,
}

impl DataSnapshot {
    pub fn related(&self, trace: &[u8], span: &[u8]) -> Vec<Arc<DataObservation>> {
        self.records
            .iter()
            .filter(|record| {
                record
                    .correlation
                    .as_ref()
                    .is_some_and(|key| key.0 == trace && key.1 == span)
            })
            .cloned()
            .collect()
    }
}

pub struct StreamDataStore {
    workflow: WorkflowId,
    run: RunId,
    streaming: bool,
    records: BTreeMap<u64, (Arc<DataObservation>, usize)>,
    payload_bytes: usize,
    witnesses: BTreeMap<u64, [u8; 32]>,
    retired_before: u64,
    next: Option<u64>,
    highest: Option<u64>,
    lost_evidence: bool,
    started: bool,
    ended: Option<u64>,
    finished: bool,
    counts: CaptureCounts,
    counts_sequence: Option<u64>,
    observed: u64,
    evicted: u64,
    old_retransmissions: u64,
    faults: u64,
    diagnostic: Option<Arc<DataStoreError>>,
    cached: Option<Arc<DataSnapshot>>,
}

impl StreamDataStore {
    pub fn new(workflow: WorkflowId, run: RunId, streaming: bool) -> Self {
        Self {
            workflow,
            run,
            streaming,
            records: BTreeMap::new(),
            payload_bytes: 0,
            witnesses: BTreeMap::new(),
            retired_before: 0,
            next: Some(0),
            highest: None,
            lost_evidence: false,
            started: false,
            ended: None,
            finished: false,
            counts: CaptureCounts::default(),
            counts_sequence: None,
            observed: 0,
            evicted: 0,
            old_retransmissions: 0,
            faults: 0,
            diagnostic: None,
            cached: None,
        }
    }

    pub fn admit_log(
        &mut self,
        log: &opentelemetry_proto::tonic::logs::v1::LogRecord,
    ) -> Result<bool, Arc<DataStoreError>> {
        match stream_data::from_log(log) {
            Ok(record) => self.admit(record),
            Err(source) => Err(self.fail(source.into())),
        }
    }

    pub fn admit(&mut self, record: DataRecord) -> Result<bool, Arc<DataStoreError>> {
        if record.workflow_id != self.workflow || record.run_id != self.run {
            return Ok(false);
        }
        let result = self.apply(record);
        result.map_err(|error| self.fail(error))
    }

    fn fail(&mut self, error: DataStoreError) -> Arc<DataStoreError> {
        self.faults = self.faults.saturating_add(1);
        self.cached = None;
        let diagnostic = self
            .diagnostic
            .get_or_insert_with(|| Arc::new(error))
            .clone();
        self.evict();
        diagnostic
    }

    pub fn note_rejection(&mut self) {
        self.fail(
            HistorySnafu {
                message: "receiver rejected a data request",
            }
            .build(),
        );
    }

    fn apply(&mut self, record: DataRecord) -> Result<bool, DataStoreError> {
        let encoded = record.encode()?;
        let sequence = record.sequence;
        let digest: [u8; 32] = Sha256::digest(&encoded.bytes).into();
        if let Some(previous) = self.witnesses.get(&sequence) {
            ensure!(previous == &digest, ConflictSnafu { sequence });
            return Ok(false);
        }
        if sequence < self.retired_before {
            self.old_retransmissions = self.old_retransmissions.saturating_add(1);
            self.cached = None;
            return Ok(false);
        }
        ensure!(
            self.ended.is_none_or(|end| sequence <= end),
            HistorySnafu {
                message: "record followed the capture end"
            }
        );
        match &record.payload {
            DataPayload::Start => {
                ensure!(
                    sequence == 0 && !self.started,
                    HistorySnafu {
                        message: "invalid capture start"
                    }
                );
                self.started = true;
            }
            DataPayload::Status { counts } | DataPayload::End { counts } => {
                if matches!(record.payload, DataPayload::End { .. }) {
                    ensure!(
                        self.ended.is_none()
                            && self.highest.is_none_or(|highest| highest <= sequence),
                        HistorySnafu {
                            message: "conflicting capture end"
                        }
                    );
                }
                if self.counts_sequence.is_none_or(|prior| sequence > prior) {
                    ensure!(
                        counts.omitted >= self.counts.omitted
                            && counts.dropped >= self.counts.dropped,
                        HistorySnafu {
                            message: "capture loss counters decreased"
                        }
                    );
                    self.counts = *counts;
                    self.counts_sequence = Some(sequence);
                }
                if matches!(record.payload, DataPayload::End { .. }) {
                    self.ended = Some(sequence);
                }
            }
            DataPayload::Ports { .. } => {
                let correlation = record.trace_context.as_ref().map(|context| {
                    (
                        native_bytes(&context.trace_id),
                        native_bytes(&context.span_id),
                    )
                });
                let observation = Arc::new(DataObservation {
                    correlation,
                    record,
                });
                let bytes = observation_bytes(&observation);
                self.observed = self.observed.saturating_add(1);
                self.payload_bytes = self.payload_bytes.saturating_add(bytes);
                self.records.insert(sequence, (observation, bytes));
            }
        }
        self.witnesses.insert(sequence, digest);
        self.highest = Some(self.highest.map_or(sequence, |prior| prior.max(sequence)));
        while let Some(next) = self.next {
            if !self.witnesses.contains_key(&next) {
                break;
            }
            self.next = next.checked_add(1);
        }
        while self.witnesses.len() > MAX_WITNESSES {
            let (retired, _) = self.witnesses.pop_first().unwrap();
            if self.next.is_some_and(|next| retired >= next) {
                self.lost_evidence = true;
            }
            self.retired_before = retired.saturating_add(1);
        }
        self.evict();
        self.cached = None;
        Ok(true)
    }

    fn evict(&mut self) {
        while self.records.len() > MAX_DATA_RECORDS || self.retained_bytes() > MAX_DATA_BYTES {
            let Some((_, (_, bytes))) = self.records.pop_first() else {
                break;
            };
            self.payload_bytes -= bytes;
            self.evicted = self.evicted.saturating_add(1);
        }
    }
    fn retained_bytes(&self) -> usize {
        self.payload_bytes
            + self.witnesses.len() * WITNESS_BYTES
            + if self.diagnostic.is_some() {
                DIAGNOSTIC_BYTES
            } else {
                0
            }
    }

    pub fn finish(&mut self) {
        self.finished = true;
        self.cached = None;
    }

    fn gaps(&self) -> Option<u64> {
        if self.lost_evidence {
            return None;
        }
        let Some(highest) = self.highest else {
            return Some(0);
        };
        let Some(next) = self.next else {
            return Some(0);
        };
        if next > highest {
            return Some(0);
        }
        let received = self.witnesses.range(next..=highest).count() as u64;
        Some(
            highest
                .saturating_sub(next)
                .saturating_add(1)
                .saturating_sub(received),
        )
    }

    pub fn snapshot(&mut self) -> Arc<DataSnapshot> {
        if let Some(cached) = &self.cached {
            return cached.clone();
        }
        let gaps = self.gaps();
        let status = if self.highest.is_none() {
            if self.finished {
                "Capture unavailable; no supported stream data received"
            } else {
                "Waiting for stream data capture"
            }
        } else if !self.started {
            "Capture start missing; history is unverified"
        } else if self.diagnostic.is_some() {
            "Capture has decoding or conflict faults"
        } else if self.lost_evidence {
            "Capture verification exceeded its retained window"
        } else if self.ended.is_some() && gaps == Some(0) && self.counts.dropped == 0 {
            if self.counts.omitted == 0 {
                "Capture ended; retained delivery verified"
            } else {
                "Capture ended with value omissions"
            }
        } else if self.ended.is_some() {
            "Capture ended with missing data"
        } else if self.finished {
            "Capture tail unverified; end record missing"
        } else {
            "Collecting stream data; tail unverified"
        };
        let snapshot = Arc::new(DataSnapshot {
            streaming: self.streaming,
            records: self
                .records
                .values()
                .map(|(record, _)| record.clone())
                .collect(),
            status: status.into(),
            observed: self.observed,
            evicted: self.evicted,
            old_retransmissions: self.old_retransmissions,
            omissions: self.counts.omitted,
            sender_drops: self.counts.dropped,
            gaps,
            faults: self.faults,
            diagnostic: self.diagnostic.clone(),
            retained_bytes: self.retained_bytes(),
        });
        self.cached = Some(snapshot.clone());
        snapshot
    }
}

fn observation_bytes(observation: &DataObservation) -> usize {
    let record = &observation.record;
    let mut bytes =
        std::mem::size_of::<DataObservation>() + 512 + record.workflow_id.as_str().len() * 2;
    if let Some(context) = &record.trace_context {
        bytes += context.trace_id.capacity() + context.span_id.capacity();
    }
    if let DataPayload::Ports {
        identity,
        port_names,
        values,
        ..
    } = &record.payload
    {
        bytes += std::mem::size_of_val(identity.as_ref())
            + identity.node.id.capacity()
            + identity.node.kind.capacity();
        bytes += identity.node.path.capacity()
            * std::mem::size_of::<mf_telemetry::event::LoopPathEntry>();
        bytes += identity
            .node
            .path
            .iter()
            .map(|entry| entry.loop_id.capacity())
            .sum::<usize>();
        bytes += port_names.capacity() * std::mem::size_of::<String>()
            + port_names.iter().map(String::capacity).sum::<usize>();
        if let Some(value) = values {
            bytes += value_bytes(value);
        }
    }
    bytes
}

fn value_bytes(value: &DataValue) -> usize {
    std::mem::size_of::<DataValue>()
        + match value {
            DataValue::String(value) => value.capacity(),
            DataValue::Array(items) => {
                items.capacity() * std::mem::size_of::<DataValue>()
                    + items.iter().map(value_bytes).sum::<usize>()
            }
            DataValue::Object(items) => items
                .iter()
                .map(|(key, value)| 256 + key.capacity() + value_bytes(value))
                .sum::<usize>(),
            _ => 0,
        }
}

fn native_bytes(text: &str) -> Vec<u8> {
    fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => unreachable!("record encoding validated canonical hexadecimal context"),
        }
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| (nibble(pair[0]) << 4) | nibble(pair[1]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_telemetry::{
        event::NodeIdentity,
        stream::{StreamIdentity, StreamTrigger},
        stream_data::{DataIdentity, Direction},
        wire::TraceContext,
    };

    fn store() -> StreamDataStore {
        StreamDataStore::new(
            WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
            RunId::try_from("12345678-1234-4234-9234-123456789abc".to_owned()).unwrap(),
            true,
        )
    }
    fn record(store: &StreamDataStore, sequence: u64, payload: DataPayload) -> DataRecord {
        DataRecord {
            version: 1,
            workflow_id: store.workflow.clone(),
            run_id: store.run,
            sequence,
            time_unix_nano: sequence,
            trace_context: None,
            payload,
        }
    }
    fn ports(store: &StreamDataStore, sequence: u64, bytes: usize) -> DataRecord {
        record(
            store,
            sequence,
            DataPayload::Ports {
                identity: Box::new(DataIdentity {
                    node: NodeIdentity {
                        id: "feed".into(),
                        kind: "producer".into(),
                        path: vec![],
                    },
                    stream: StreamIdentity {
                        invocation: 1,
                        trigger: StreamTrigger::Startup,
                        message: None,
                        parent: None,
                    },
                    output: None,
                    ordinal: Some(sequence),
                }),
                direction: Direction::Output,
                port_names: vec!["value".into()],
                ports_truncated: false,
                values: Some(DataValue::Object(BTreeMap::from([(
                    "value".into(),
                    DataValue::String("x".repeat(bytes)),
                )]))),
                omission: None,
            },
        )
    }
    #[test]
    fn independent_out_of_order_rows_and_exact_native_correlation() {
        let mut store = store();
        let mut second = ports(&store, 2, 1);
        second.trace_context = Some(TraceContext {
            trace_id: "01".repeat(16),
            span_id: "02".repeat(8),
            trace_flags: 0,
        });
        store.admit(second.clone()).unwrap();
        let first = ports(&store, 1, 1);
        store.admit(first).unwrap();
        assert_eq!(
            store
                .snapshot()
                .records
                .iter()
                .map(|row| row.record.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(store.snapshot().related(&[1; 16], &[2; 8]).len(), 1);
        assert_eq!(store.snapshot().gaps, Some(1));
        assert!(!store.admit(second.clone()).unwrap());
        second.time_unix_nano += 1;
        assert!(matches!(
            &*store.admit(second).unwrap_err(),
            DataStoreError::Conflict { sequence: 2 }
        ));
        let mut unrelated = ports(&store, 3, 1);
        unrelated.run_id = RunId::new();
        assert!(!store.admit(unrelated).unwrap());
        assert_eq!(store.snapshot().records.len(), 2);
    }
    #[test]
    fn end_evidence_repairs_gaps_and_keeps_loss_classes_separate() {
        let mut store = store();
        store.admit(record(&store, 0, DataPayload::Start)).unwrap();
        store
            .admit(record(
                &store,
                3,
                DataPayload::End {
                    counts: CaptureCounts {
                        omitted: 1,
                        dropped: 1,
                    },
                },
            ))
            .unwrap();
        store.admit(ports(&store, 2, 1)).unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.gaps, Some(1));
        assert_eq!(snapshot.sender_drops, 1);
        assert_eq!(snapshot.omissions, 1);
        store.admit(ports(&store, 1, 1)).unwrap();
        assert_eq!(store.snapshot().gaps, Some(0));
        assert!(store.snapshot().status.contains("missing data"));
        assert!(store.admit(ports(&store, 4, 1)).is_err());
        assert_eq!(store.snapshot().records.len(), 2);
    }
    #[test]
    fn absence_and_invalid_end_never_claim_complete_capture() {
        let mut store = store();
        store.finish();
        assert!(store.snapshot().status.contains("unavailable"));
        store.admit(ports(&store, 5, 1)).unwrap();
        assert!(store.snapshot().status.contains("start missing"));
        assert!(
            store
                .admit(record(
                    &store,
                    4,
                    DataPayload::End {
                        counts: CaptureCounts {
                            omitted: 99,
                            dropped: 99
                        }
                    }
                ))
                .is_err()
        );
        assert_eq!(store.snapshot().sender_drops, 0);
        store.admit(record(&store, 0, DataPayload::Start)).unwrap();
        assert!(store.snapshot().status.contains("faults"));
        let mut clean = super::tests::store();
        clean.admit(record(&clean, 0, DataPayload::Start)).unwrap();
        clean.finish();
        assert!(clean.snapshot().status.contains("end record missing"));
    }
    #[test]
    fn sustained_retention_releases_payloads_and_rejects_ancient_retries() {
        let mut store = store();
        store.admit(record(&store, 0, DataPayload::Start)).unwrap();
        let old = ports(&store, 1, 1);
        store.admit(old.clone()).unwrap();
        let weak = Arc::downgrade(&store.snapshot().records[0]);
        for sequence in 2..=10_000 {
            store.admit(ports(&store, sequence, 1)).unwrap();
        }
        assert!(weak.upgrade().is_none());
        let snapshot = store.snapshot();
        assert_eq!(snapshot.records.len(), MAX_DATA_RECORDS);
        assert!(snapshot.retained_bytes <= MAX_DATA_BYTES);
        assert_eq!(snapshot.evicted, 10_000 - MAX_DATA_RECORDS as u64);
        assert_eq!(snapshot.gaps, Some(0));
        assert!(!store.admit(old).unwrap());
        assert_eq!(store.snapshot().old_retransmissions, 1);
        store
            .admit(record(
                &store,
                10_001,
                DataPayload::End {
                    counts: CaptureCounts::default(),
                },
            ))
            .unwrap();
        assert!(store.snapshot().status.contains("verified"));
    }
    #[test]
    fn byte_budget_and_gap_window_are_bounded() {
        let mut store = store();
        for sequence in 1..=5000 {
            store.admit(ports(&store, sequence, 1)).unwrap();
        }
        assert_eq!(store.snapshot().gaps, None);
        assert!(store.snapshot().status.contains("start missing"));
        for sequence in 5001..=5200 {
            store.admit(ports(&store, sequence, 200_000)).unwrap();
        }
        assert!(store.snapshot().retained_bytes <= MAX_DATA_BYTES);
        assert!(store.snapshot().records.len() < 100);
        assert!(store.witnesses.len() <= MAX_WITNESSES);
        let mut extreme = super::tests::store();
        extreme.admit(ports(&extreme, u64::MAX, 1)).unwrap();
        assert!(extreme.snapshot().gaps.unwrap() > 0);
        assert_eq!(extreme.witnesses.len(), 1);
    }
    #[test]
    fn malformed_records_keep_typed_sources_and_later_data() {
        let mut store = store();
        let error = store
            .admit_log(&opentelemetry_proto::tonic::logs::v1::LogRecord::default())
            .unwrap_err();
        assert!(matches!(
            &*error,
            DataStoreError::Decode {
                source: DataError::Invalid { .. }
            }
        ));
        store.admit(ports(&store, 1, 1)).unwrap();
        let mut unsupported = ports(&store, 2, 1);
        unsupported.version = 99;
        assert!(store.admit(unsupported).is_err());
        assert!(Arc::ptr_eq(
            &error,
            store.snapshot().diagnostic.as_ref().unwrap()
        ));
        assert_eq!(store.snapshot().faults, 2);
        assert_eq!(store.snapshot().records.len(), 1);
        assert!(store.snapshot().retained_bytes <= MAX_DATA_BYTES);
    }
}
