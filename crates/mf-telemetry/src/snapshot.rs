//! Structured snapshot records carried by OTLP logs.
use opentelemetry::logs::{AnyValue as SdkValue, LogRecord as SdkRecord};
use opentelemetry_proto::tonic::{
    common::v1::{AnyValue, ArrayValue, KeyValue, KeyValueList, any_value},
    logs::v1::LogRecord,
};
use prost::Message;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

pub const SCOPE: &str = "mf.snapshot";
pub const RECORD_EVENT: &str = "mf.snapshot.record";
pub const FRAGMENT_EVENT: &str = "mf.snapshot.fragment";
pub const SCHEMA_VERSION: i64 = 1;
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_PENDING_RECORDS: usize = 1024;

#[derive(Debug, Snafu)]
pub enum SnapshotError {
    #[snafu(display("{message}"))]
    Invalid { message: String },
    #[snafu(display("{message}: {source}"))]
    Integer {
        message: &'static str,
        source: std::num::TryFromIntError,
    },
    #[snafu(display("invalid snapshot digest: {source}"))]
    Digest {
        source: std::array::TryFromSliceError,
    },
    #[snafu(display("invalid snapshot protobuf: {source}"))]
    Protobuf { source: prost::DecodeError },
}

fn digest(value: &Value) -> [u8; 32] {
    fn hash(value: &Value, state: &mut Sha256) {
        match value {
            Value::Null => state.update([0]),
            Value::Bool(value) => state.update([1, u8::from(*value)]),
            Value::Number(value) => {
                if value.is_f64() {
                    state.update([2]);
                    state.update(value.as_f64().unwrap().to_bits().to_le_bytes());
                } else if let Some(value) = value.as_i64() {
                    state.update([6]);
                    state.update(value.to_le_bytes());
                } else {
                    state.update([7]);
                    state.update(value.as_u64().unwrap().to_le_bytes());
                }
            }
            Value::String(value) => {
                state.update([3]);
                state.update((value.len() as u64).to_le_bytes());
                state.update(value.as_bytes());
            }
            Value::Array(values) => {
                state.update([4]);
                state.update((values.len() as u64).to_le_bytes());
                for value in values {
                    hash(value, state);
                }
            }
            Value::Object(values) => {
                state.update([5]);
                state.update((values.len() as u64).to_le_bytes());
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_by_key(|(key, _)| *key);
                for (key, value) in entries {
                    state.update((key.len() as u64).to_le_bytes());
                    state.update(key.as_bytes());
                    hash(value, state);
                }
            }
        }
    }
    let mut state = Sha256::new();
    hash(value, &mut state);
    state.finalize().into()
}

pub struct Packet {
    sequence: usize,
    digest: [u8; 32],
    event_name: &'static str,
    body: AnyValue,
}

pub enum Packets {
    Record(Option<Packet>),
    Fragments {
        sequence: usize,
        digest: [u8; 32],
        encoded: Vec<u8>,
        index: usize,
    },
}

pub fn packets(sequence: usize, body: Value) -> Result<Packets, SnapshotError> {
    i64::try_from(sequence).context(IntegerSnafu {
        message: "snapshot sequence exceeds OTLP integer range",
    })?;
    let digest = digest(&body);
    let body = encode(body, 0)?;
    if body.encoded_len() <= CHUNK_BYTES {
        Ok(Packets::Record(Some(Packet {
            sequence,
            digest,
            event_name: RECORD_EVENT,
            body,
        })))
    } else {
        Ok(Packets::Fragments {
            sequence,
            digest,
            encoded: body.encode_to_vec(),
            index: 0,
        })
    }
}

impl Iterator for Packets {
    type Item = Packet;
    fn next(&mut self) -> Option<Packet> {
        match self {
            Self::Record(packet) => packet.take(),
            Self::Fragments {
                sequence,
                digest,
                encoded,
                index,
            } => {
                let total = encoded.len().div_ceil(CHUNK_BYTES);
                if *index == total {
                    return None;
                }
                let start = *index * CHUNK_BYTES;
                let payload = encoded[start..(start + CHUNK_BYTES).min(encoded.len())].to_vec();
                let body = object([
                    ("index", integer(*index as i64)),
                    ("total", integer(total as i64)),
                    (
                        "payload",
                        AnyValue {
                            value: Some(any_value::Value::BytesValue(payload)),
                        },
                    ),
                ]);
                *index += 1;
                Some(Packet {
                    sequence: *sequence,
                    digest: *digest,
                    event_name: FRAGMENT_EVENT,
                    body,
                })
            }
        }
    }
}

impl Packet {
    pub fn into_log_record(self, workflow: &str, run: &str) -> LogRecord {
        LogRecord {
            time_unix_nano: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            event_name: self.event_name.into(),
            body: Some(self.body),
            attributes: vec![
                pair("mf.workflow.id", string(workflow)),
                pair("mf.run.id", string(run)),
                pair("mf.snapshot.version", integer(SCHEMA_VERSION)),
                pair("mf.snapshot.sequence", integer(self.sequence as i64)),
                pair(
                    "mf.snapshot.digest",
                    AnyValue {
                        value: Some(any_value::Value::BytesValue(self.digest.to_vec())),
                    },
                ),
            ],
            ..LogRecord::default()
        }
    }

    pub fn write_to(
        self,
        record: &mut impl SdkRecord,
        workflow: &str,
        run: &str,
    ) -> Result<(), SnapshotError> {
        let event_name = self.event_name;
        let wire = self.into_log_record(workflow, run);
        record.set_event_name(event_name);
        record.set_timestamp(UNIX_EPOCH + std::time::Duration::from_nanos(wire.time_unix_nano));
        record.set_body(sdk_value(wire.body.unwrap())?);
        for attribute in wire.attributes {
            record.add_attribute(attribute.key, sdk_value(attribute.value.unwrap())?);
        }
        Ok(())
    }

    fn from_log(record: LogRecord) -> Result<Self, SnapshotError> {
        ensure!(
            record.dropped_attributes_count == 0,
            InvalidSnafu {
                message: "snapshot attributes were dropped"
            }
        );
        let mut attributes = BTreeMap::new();
        for attribute in record.attributes {
            ensure!(
                attribute.key_strindex == 0,
                InvalidSnafu {
                    message: "indexed snapshot attributes are not supported"
                }
            );
            ensure!(
                attributes.insert(attribute.key, attribute.value).is_none(),
                InvalidSnafu {
                    message: "duplicate snapshot attribute"
                }
            );
        }
        let attr = |name: &str| {
            attributes
                .get(name)
                .and_then(Option::as_ref)
                .with_context(|| InvalidSnafu {
                    message: format!("missing snapshot attribute {name}"),
                })
        };
        ensure!(
            read_int(attr("mf.snapshot.version")?)? == SCHEMA_VERSION,
            InvalidSnafu {
                message: "unsupported snapshot transport version"
            }
        );
        let sequence =
            usize::try_from(read_int(attr("mf.snapshot.sequence")?)?).context(IntegerSnafu {
                message: "invalid snapshot sequence",
            })?;
        let digest: [u8; 32] = match &attr("mf.snapshot.digest")?.value {
            Some(any_value::Value::BytesValue(value)) => {
                value.as_slice().try_into().context(DigestSnafu)?
            }
            _ => {
                return InvalidSnafu {
                    message: "invalid snapshot digest",
                }
                .fail();
            }
        };
        let body = record.body.context(InvalidSnafu {
            message: "missing snapshot body",
        })?;
        ensure!(
            body.encoded_len() <= CHUNK_BYTES + 512,
            InvalidSnafu {
                message: "snapshot packet exceeded the size limit"
            }
        );
        let event_name = match record.event_name.as_str() {
            RECORD_EVENT => RECORD_EVENT,
            FRAGMENT_EVENT => FRAGMENT_EVENT,
            _ => {
                return InvalidSnafu {
                    message: "unknown snapshot event",
                }
                .fail();
            }
        };
        Ok(Self {
            sequence,
            digest,
            event_name,
            body,
        })
    }
}

struct PartialRecord {
    digest: [u8; 32],
    contents: Contents,
}

enum Contents {
    Complete(Value),
    Fragments {
        total: usize,
        parts: BTreeMap<usize, Vec<u8>>,
    },
}

#[derive(Default)]
pub struct Assembler {
    applied: Vec<[u8; 32]>,
    pending: BTreeMap<usize, PartialRecord>,
}

impl Assembler {
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn next_sequence(&self) -> usize {
        self.applied.len()
    }

    pub fn push(&mut self, record: LogRecord) -> Result<Vec<Value>, SnapshotError> {
        let Packet {
            sequence,
            digest: expected,
            event_name,
            body,
        } = Packet::from_log(record)?;
        if let Some(previous) = self.applied.get(sequence) {
            ensure!(
                previous == &expected,
                InvalidSnafu {
                    message: "conflicting snapshot retransmission"
                }
            );
            if event_name == RECORD_EVENT {
                verify(&decode(body, 0)?, &expected)?;
            }
            return Ok(Vec::new());
        }
        ensure!(
            self.pending.contains_key(&sequence) || self.pending.len() != MAX_PENDING_RECORDS,
            InvalidSnafu {
                message: "snapshot sequence gap exceeded the pending record limit"
            }
        );
        ensure!(
            self.pending
                .get(&sequence)
                .is_none_or(|record| record.digest == expected),
            InvalidSnafu {
                message: "conflicting snapshot retransmission"
            }
        );
        if event_name == RECORD_EVENT {
            let value = decode(body, 0)?;
            verify(&value, &expected)?;
            self.pending.insert(
                sequence,
                PartialRecord {
                    digest: expected,
                    contents: Contents::Complete(value),
                },
            );
        } else {
            let mut fields = fields(body)?;
            let index = usize::try_from(read_int(&fields.remove("index").context(
                InvalidSnafu {
                    message: "missing fragment index",
                },
            )?)?)
            .context(IntegerSnafu {
                message: "invalid fragment index",
            })?;
            let total = usize::try_from(read_int(&fields.remove("total").context(
                InvalidSnafu {
                    message: "missing fragment total",
                },
            )?)?)
            .context(IntegerSnafu {
                message: "invalid fragment total",
            })?;
            let payload = match fields.remove("payload").and_then(|value| value.value) {
                Some(any_value::Value::BytesValue(payload)) => payload,
                _ => {
                    return InvalidSnafu {
                        message: "invalid snapshot fragment payload",
                    }
                    .fail();
                }
            };
            ensure!(
                fields.is_empty()
                    && index < total
                    && !payload.is_empty()
                    && payload.len() <= CHUNK_BYTES
                    && (index + 1 >= total || payload.len() == CHUNK_BYTES),
                InvalidSnafu {
                    message: "invalid snapshot fragment bounds"
                }
            );
            let partial = self
                .pending
                .entry(sequence)
                .or_insert_with(|| PartialRecord {
                    digest: expected,
                    contents: Contents::Fragments {
                        total,
                        parts: BTreeMap::new(),
                    },
                });
            if let Contents::Fragments {
                total: prior_total,
                parts,
            } = &mut partial.contents
            {
                ensure!(
                    *prior_total == total
                        && parts.get(&index).is_none_or(|prior| prior == &payload),
                    InvalidSnafu {
                        message: "conflicting snapshot fragment"
                    }
                );
                parts.insert(index, payload);
                if parts.len() == total {
                    let encoded: Vec<_> = parts
                        .values()
                        .flat_map(|part| part.iter().copied())
                        .collect();
                    let value = AnyValue::decode(encoded.as_slice()).context(ProtobufSnafu)?;
                    let value = decode(value, 0)?;
                    verify(&value, &expected)?;
                    partial.contents = Contents::Complete(value);
                }
            }
        }
        let mut ready = Vec::new();
        while self
            .pending
            .get(&self.applied.len())
            .is_some_and(|record| matches!(record.contents, Contents::Complete(_)))
        {
            let record = self.pending.remove(&self.applied.len()).unwrap();
            let Contents::Complete(value) = record.contents else {
                unreachable!()
            };
            self.applied.push(record.digest);
            ready.push(value);
        }
        Ok(ready)
    }
}

fn verify(value: &Value, expected: &[u8; 32]) -> Result<(), SnapshotError> {
    if &digest(value) == expected {
        Ok(())
    } else {
        InvalidSnafu {
            message: "snapshot digest mismatch",
        }
        .fail()
    }
}
fn pair(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(value),
        ..KeyValue::default()
    }
}
fn integer(value: i64) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::IntValue(value)),
    }
}
fn string(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.into())),
    }
}
fn object<const N: usize>(items: [(&str, AnyValue); N]) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::KvlistValue(KeyValueList {
            values: items
                .into_iter()
                .map(|(key, value)| pair(key, value))
                .collect(),
        })),
    }
}
fn read_int(value: &AnyValue) -> Result<i64, SnapshotError> {
    match value.value {
        Some(any_value::Value::IntValue(value)) => Ok(value),
        _ => InvalidSnafu {
            message: "expected OTLP integer",
        }
        .fail(),
    }
}
fn fields(value: AnyValue) -> Result<BTreeMap<String, AnyValue>, SnapshotError> {
    let Some(any_value::Value::KvlistValue(map)) = value.value else {
        return InvalidSnafu {
            message: "expected OTLP map",
        }
        .fail();
    };
    let mut fields = BTreeMap::new();
    for pair in map.values {
        ensure!(
            pair.key_strindex == 0,
            InvalidSnafu {
                message: "indexed snapshot keys are not supported"
            }
        );
        if fields
            .insert(
                pair.key,
                pair.value.context(InvalidSnafu {
                    message: "missing OTLP map value",
                })?,
            )
            .is_some()
        {
            return InvalidSnafu {
                message: "duplicate OTLP map key",
            }
            .fail();
        }
    }
    Ok(fields)
}
fn encode(value: Value, depth: usize) -> Result<AnyValue, SnapshotError> {
    ensure!(
        depth <= 16,
        InvalidSnafu {
            message: "snapshot envelope nesting exceeded the limit"
        }
    );
    let value = match value {
        Value::Null => None,
        Value::Bool(value) => Some(any_value::Value::BoolValue(value)),
        Value::String(value) => Some(any_value::Value::StringValue(value)),
        Value::Number(value) => Some(if value.is_f64() {
            any_value::Value::DoubleValue(value.as_f64().context(InvalidSnafu {
                message: "invalid floating-point value",
            })?)
        } else {
            any_value::Value::IntValue(value.as_i64().context(InvalidSnafu {
                message: "OTLP integer exceeds signed 64-bit range",
            })?)
        }),
        Value::Array(values) => Some(any_value::Value::ArrayValue(ArrayValue {
            values: values
                .into_iter()
                .map(|value| encode(value, depth + 1))
                .collect::<Result<_, _>>()?,
        })),
        Value::Object(values) => Some(any_value::Value::KvlistValue(KeyValueList {
            values: values
                .into_iter()
                .map(|(key, value)| {
                    Ok(KeyValue {
                        key,
                        value: Some(encode(value, depth + 1)?),
                        ..KeyValue::default()
                    })
                })
                .collect::<Result<_, SnapshotError>>()?,
        })),
    };
    Ok(AnyValue { value })
}
fn decode(value: AnyValue, depth: usize) -> Result<Value, SnapshotError> {
    ensure!(
        depth <= 16,
        InvalidSnafu {
            message: "snapshot envelope nesting exceeded the limit"
        }
    );
    Ok(match value.value {
        None => Value::Null,
        Some(any_value::Value::StringValue(value)) => Value::String(value),
        Some(any_value::Value::BoolValue(value)) => Value::Bool(value),
        Some(any_value::Value::IntValue(value)) => Value::from(value),
        Some(any_value::Value::DoubleValue(value)) => {
            Value::Number(serde_json::Number::from_f64(value).context(InvalidSnafu {
                message: "non-finite snapshot number",
            })?)
        }
        Some(any_value::Value::ArrayValue(values)) => Value::Array(
            values
                .values
                .into_iter()
                .map(|value| decode(value, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Some(any_value::Value::KvlistValue(values)) => {
            let mut map = Map::new();
            for pair in values.values {
                ensure!(
                    pair.key_strindex == 0,
                    InvalidSnafu {
                        message: "indexed snapshot keys are not supported"
                    }
                );
                if map
                    .insert(
                        pair.key,
                        decode(
                            pair.value.context(InvalidSnafu {
                                message: "missing OTLP map value",
                            })?,
                            depth + 1,
                        )?,
                    )
                    .is_some()
                {
                    return InvalidSnafu {
                        message: "duplicate OTLP map key",
                    }
                    .fail();
                }
            }
            Value::Object(map)
        }
        _ => {
            return InvalidSnafu {
                message: "unsupported snapshot body value",
            }
            .fail();
        }
    })
}
fn sdk_value(value: AnyValue) -> Result<SdkValue, SnapshotError> {
    Ok(match value.value {
        Some(any_value::Value::StringValue(value)) => SdkValue::String(value.into()),
        Some(any_value::Value::BoolValue(value)) => SdkValue::Boolean(value),
        Some(any_value::Value::IntValue(value)) => SdkValue::Int(value),
        Some(any_value::Value::DoubleValue(value)) => SdkValue::Double(value),
        Some(any_value::Value::BytesValue(value)) => SdkValue::Bytes(Box::new(value)),
        Some(any_value::Value::ArrayValue(values)) => SdkValue::ListAny(Box::new(
            values
                .values
                .into_iter()
                .map(sdk_value)
                .collect::<Result<_, _>>()?,
        )),
        Some(any_value::Value::KvlistValue(values)) => SdkValue::Map(Box::new(
            values
                .values
                .into_iter()
                .map(|pair| {
                    Ok((
                        pair.key.into(),
                        sdk_value(pair.value.context(InvalidSnafu {
                            message: "missing OTLP value",
                        })?)?,
                    ))
                })
                .collect::<Result<_, SnapshotError>>()?,
        )),
        None => {
            return InvalidSnafu {
                message: "snapshot envelopes must omit absent optional fields",
            }
            .fail();
        }
        _ => {
            return InvalidSnafu {
                message: "indexed OTLP strings are not supported in snapshot envelopes",
            }
            .fail();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_fragment_protobuf_preserves_the_decode_error() {
        use std::error::Error;
        let body = object([
            ("index", integer(0)),
            ("total", integer(1)),
            (
                "payload",
                AnyValue {
                    value: Some(any_value::Value::BytesValue(vec![0xff])),
                },
            ),
        ]);
        let record = Packet {
            sequence: 0,
            digest: [0; 32],
            event_name: FRAGMENT_EVENT,
            body,
        }
        .into_log_record("workflow", "run");
        let error = Assembler::default().push(record).unwrap_err();
        assert!(error.source().unwrap().is::<prost::DecodeError>());
    }

    #[test]
    fn invalid_snapshot_metadata_preserves_conversion_errors() {
        use std::error::Error;
        let record = || {
            packets(0, json!({"record":"header","version":1}))
                .unwrap()
                .next()
                .unwrap()
                .into_log_record("workflow", "run")
        };
        let mut negative_sequence = record();
        negative_sequence
            .attributes
            .iter_mut()
            .find(|attribute| attribute.key == "mf.snapshot.sequence")
            .unwrap()
            .value = Some(integer(-1));
        let error = Assembler::default().push(negative_sequence).unwrap_err();
        assert!(error.source().unwrap().is::<std::num::TryFromIntError>());
        let mut short_digest = record();
        short_digest
            .attributes
            .iter_mut()
            .find(|attribute| attribute.key == "mf.snapshot.digest")
            .unwrap()
            .value = Some(AnyValue {
            value: Some(any_value::Value::BytesValue(vec![0])),
        });
        let error = Assembler::default().push(short_digest).unwrap_err();
        assert!(
            error
                .source()
                .unwrap()
                .is::<std::array::TryFromSliceError>()
        );
    }

    #[test]
    fn structured_records_are_reordered_and_retransmissions_are_idempotent() {
        let first = packets(0, json!({"record":"header","version":1}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        let second = packets(1, json!({"record":"value","id":0,"value":{"kind":"null"}}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        assert!(matches!(
            first.body.as_ref().unwrap().value,
            Some(any_value::Value::KvlistValue(_))
        ));
        let mut assembler = Assembler::default();
        assert!(assembler.push(second.clone()).unwrap().is_empty());
        assert!(assembler.has_pending());
        assert_eq!(assembler.push(first.clone()).unwrap().len(), 2);
        assert!(!assembler.has_pending());
        assert!(assembler.push(first).unwrap().is_empty());
        assert!(assembler.push(second).unwrap().is_empty());
        assert_eq!(assembler.next_sequence(), 2);
    }

    #[test]
    fn large_values_use_bounded_protobuf_fragments_without_a_json_string_envelope() {
        let original = json!({"record":"value","id":0,"value":{"kind":"string","data":"\u{754c}".repeat(CHUNK_BYTES)}});
        let mut logs: Vec<_> = packets(0, original.clone())
            .unwrap()
            .map(|packet| packet.into_log_record("workflow", "run"))
            .collect();
        assert!(logs.len() > 1);
        assert!(
            logs.iter()
                .all(|log| log.encoded_len() < CHUNK_BYTES + 1024)
        );
        logs.reverse();
        let mut assembler = Assembler::default();
        let first = logs[0].clone();
        assert!(assembler.push(first.clone()).unwrap().is_empty());
        assert!(assembler.push(first).unwrap().is_empty());
        let decoded: Vec<_> = logs
            .into_iter()
            .flat_map(|log| assembler.push(log).unwrap())
            .collect();
        assert_eq!(decoded, vec![original]);
    }

    #[test]
    fn conflicts_and_corrupt_fragments_are_rejected() {
        let mut assembler = Assembler::default();
        let original = packets(0, json!({"record":"header","version":1}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        assembler.push(original).unwrap();
        let conflict = packets(0, json!({"record":"header","version":2}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        assert!(
            assembler
                .push(conflict)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        let mut record = packets(1, json!({"record":"end"}))
            .unwrap()
            .next()
            .unwrap()
            .into_log_record("workflow", "run");
        record.body = Some(string("corrupted"));
        assert!(
            assembler
                .push(record)
                .unwrap_err()
                .to_string()
                .contains("digest")
        );
    }
}
