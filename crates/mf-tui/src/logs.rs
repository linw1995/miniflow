//! Bounded retention of session LogRecords, independent of lifecycle reduction.

use opentelemetry_proto::tonic::{
    common::v1::InstrumentationScope, logs::v1::LogRecord, resource::v1::Resource,
};
use prost::Message;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

const MAX_LOG_RECORDS: usize = 4096;
const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LogObservation {
    pub id: usize,
    pub record: LogRecord,
    pub resource: Option<Arc<Resource>>,
    pub scope: Option<Arc<InstrumentationScope>>,
}

#[derive(Clone, Debug, Default)]
pub struct LogSnapshot {
    pub records: Vec<Arc<LogObservation>>,
    pub observed_records: u64,
    pub local_drops: u64,
}

impl LogSnapshot {
    pub fn related(&self, trace_id: &[u8], span_id: &[u8]) -> Vec<Arc<LogObservation>> {
        if trace_id.len() != 16 || span_id.len() != 8 {
            return Vec::new();
        }
        self.records
            .iter()
            .filter(|observation| {
                observation.record.trace_id == trace_id && observation.record.span_id == span_id
            })
            .cloned()
            .collect()
    }
}

#[derive(Default)]
pub struct LogStore {
    records: Vec<Arc<LogObservation>>,
    fingerprints: BTreeSet<[u8; 32]>,
    bytes: usize,
    observed_records: u64,
    local_drops: u64,
    cached: Option<Arc<LogSnapshot>>,
}

impl LogStore {
    pub fn admit(
        &mut self,
        record: LogRecord,
        resource: Option<Arc<Resource>>,
        scope: Option<Arc<InstrumentationScope>>,
    ) {
        self.observed_records = self.observed_records.saturating_add(1);
        self.cached = None;
        let encoded = record.encode_to_vec();
        let resource_bytes = resource
            .as_ref()
            .map(|resource| resource.encode_to_vec())
            .unwrap_or_default();
        let scope_bytes = scope
            .as_ref()
            .map(|scope| scope.encode_to_vec())
            .unwrap_or_default();
        let mut hash = Sha256::new();
        for part in [&encoded, &resource_bytes, &scope_bytes] {
            hash.update(part.len().to_le_bytes());
            hash.update(part);
        }
        let fingerprint = hash.finalize().into();
        if self.fingerprints.contains(&fingerprint) {
            return;
        }
        let bytes = encoded.len() + resource_bytes.len() + scope_bytes.len();
        if self.records.len() >= MAX_LOG_RECORDS || bytes > MAX_LOG_BYTES.saturating_sub(self.bytes)
        {
            self.local_drops = self.local_drops.saturating_add(1);
            return;
        }
        self.bytes += bytes;
        self.fingerprints.insert(fingerprint);
        self.records.push(Arc::new(LogObservation {
            id: self.records.len(),
            record,
            resource,
            scope,
        }));
    }

    pub fn snapshot(&mut self) -> Arc<LogSnapshot> {
        if let Some(snapshot) = &self.cached {
            return snapshot.clone();
        }
        let mut records = self.records.clone();
        records.sort_by_key(|observation| {
            let record = &observation.record;
            (
                if record.time_unix_nano == 0 {
                    record.observed_time_unix_nano
                } else {
                    record.time_unix_nano
                },
                observation.id,
            )
        });
        let snapshot = Arc::new(LogSnapshot {
            records,
            observed_records: self.observed_records,
            local_drops: self.local_drops,
        });
        self.cached = Some(snapshot.clone());
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};

    fn record(time: u64, trace: u8, span: u8) -> LogRecord {
        LogRecord {
            time_unix_nano: time,
            trace_id: vec![trace; 16],
            span_id: vec![span; 8],
            event_name: "test.log".into(),
            ..Default::default()
        }
    }

    #[test]
    fn retains_ordered_logs_without_requiring_spans_and_deduplicates_retries() {
        let mut store = LogStore::default();
        store.admit(record(20, 1, 2), None, None);
        store.admit(record(10, 1, 1), None, None);
        store.admit(record(20, 1, 2), None, None);
        store.admit(record(30, 2, 1), None, None);
        let snapshot = store.snapshot();
        assert_eq!(
            snapshot
                .records
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            [1, 0, 2]
        );
        assert_eq!(snapshot.related(&[1; 16], &[1; 8]).len(), 1);
        assert_eq!(snapshot.observed_records, 4);
        assert_eq!(snapshot.local_drops, 0);
        store.admit(
            LogRecord {
                observed_time_unix_nano: 15,
                ..Default::default()
            },
            None,
            None,
        );
        assert_eq!(
            store.snapshot().records[1].record.observed_time_unix_nano,
            15
        );
        assert_eq!(snapshot.records.len(), 3);
    }

    #[test]
    fn fingerprint_includes_scope_and_resource_metadata() {
        let mut store = LogStore::default();
        for name in ["first", "second"] {
            store.admit(
                record(10, 1, 1),
                Some(Arc::new(Resource::default())),
                Some(Arc::new(InstrumentationScope {
                    name: name.into(),
                    ..Default::default()
                })),
            );
        }
        store.admit(record(10, 1, 1), None, None);
        for name in ["first-service", "second-service"] {
            store.admit(
                record(10, 1, 1),
                Some(Arc::new(Resource {
                    attributes: vec![opentelemetry_proto::tonic::common::v1::KeyValue {
                        key: "service.name".into(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(name.into())),
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                None,
            );
        }
        assert_eq!(store.snapshot().records.len(), 5);
    }

    #[test]
    fn bounds_count_and_bytes_without_discarding_admitted_records() {
        let mut store = LogStore::default();
        for time in 0..MAX_LOG_RECORDS {
            store.admit(record(time as u64, 1, 1), None, None);
        }
        store.admit(record(MAX_LOG_RECORDS as u64, 1, 1), None, None);
        store.admit(record(0, 1, 1), None, None);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.records.len(), MAX_LOG_RECORDS);
        assert_eq!(snapshot.local_drops, 1);
        let mut store = LogStore::default();
        let mut oversized = record(1, 1, 1);
        oversized.body = Some(AnyValue {
            value: Some(any_value::Value::StringValue("x".repeat(MAX_LOG_BYTES))),
        });
        store.admit(oversized, None, None);
        store.admit(record(2, 1, 1), None, None);
        assert_eq!(store.snapshot().records.len(), 1);
        assert_eq!(store.snapshot().local_drops, 1);
    }
}
