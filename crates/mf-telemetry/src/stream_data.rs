//! Independently decodable, bounded business data for streaming observations.

use crate::{
    event::NodeIdentity,
    identity::{RunId, WorkflowId},
    stream::{StreamIdentity, StreamMessage, StreamTrigger, counter, optional_counter},
    wire::TraceContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Arc,
};

pub const SCOPE: &str = "mf.stream.data";
pub const RECORD_EVENT: &str = "mf.stream.data.record";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_RECORD_BYTES: usize = 256 * 1024;
pub const MAX_VALUE_DEPTH: usize = 48;
pub const MAX_QUEUE_RECORDS: usize = 128;
pub const MAX_QUEUE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub enum DataError {
    #[snafu(display("invalid stream data: {message}"))]
    Invalid { message: &'static str },
    #[snafu(display("stream data exceeded its byte budget"))]
    #[snafu(visibility(pub))]
    TooLarge,
    #[snafu(display("stream data exceeded its depth budget"))]
    #[snafu(visibility(pub))]
    TooDeep,
    #[snafu(display("stream data sequence exhausted"))]
    #[snafu(visibility(pub))]
    Exhausted,
    #[snafu(display("invalid stream data JSON: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(transparent)]
    Contract { source: crate::ContractError },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DataValue {
    Null,
    Bool(bool),
    Number(#[serde(with = "number_text")] Number),
    String(String),
    Array(Vec<DataValue>),
    Object(BTreeMap<String, DataValue>),
}

impl DataValue {
    pub fn validate(&self, depth: usize) -> Result<(), DataError> {
        ensure!(depth <= MAX_VALUE_DEPTH, TooDeepSnafu);
        match self {
            Self::Array(items) => {
                for value in items {
                    value.validate(depth + 1)?;
                }
            }
            Self::Object(items) => {
                for value in items.values() {
                    value.validate(depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Number(value) => Value::Number(value.clone()),
            Self::String(value) => Value::String(value.clone()),
            Self::Array(items) => Value::Array(items.iter().map(Self::to_json).collect()),
            Self::Object(items) => Value::Object(
                items
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }
}

mod number_text {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_json::Number;
    pub fn serialize<S: Serializer>(value: &Number, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Number, D::Error> {
        serde_json::from_str(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Omission {
    Bytes,
    Depth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataIdentity {
    pub node: NodeIdentity,
    pub stream: StreamIdentity,
    pub output: Option<StreamMessage>,
    #[serde(with = "optional_counter")]
    pub ordinal: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCounts {
    #[serde(with = "counter")]
    pub omitted: u64,
    #[serde(with = "counter")]
    pub dropped: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DataPayload {
    Start,
    Ports {
        identity: Box<DataIdentity>,
        direction: Direction,
        port_names: Vec<String>,
        ports_truncated: bool,
        values: Option<DataValue>,
        omission: Option<Omission>,
    },
    Status {
        counts: CaptureCounts,
    },
    End {
        counts: CaptureCounts,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRecord {
    pub version: u32,
    pub workflow_id: WorkflowId,
    pub run_id: RunId,
    #[serde(with = "counter")]
    pub sequence: u64,
    pub time_unix_nano: u64,
    pub trace_context: Option<TraceContext>,
    pub payload: DataPayload,
}

impl DataRecord {
    pub fn validate(&self) -> Result<(), DataError> {
        ensure!(
            self.version == SCHEMA_VERSION,
            InvalidSnafu {
                message: "unsupported version"
            }
        );
        if let Some(context) = &self.trace_context {
            context.validate()?;
        }
        if let DataPayload::Ports {
            identity,
            direction,
            port_names,
            values,
            omission,
            ..
        } = &self.payload
        {
            ensure!(
                port_names.len() <= 32 && port_names.iter().all(|name| name.len() <= 256),
                InvalidSnafu {
                    message: "port summary exceeded its limit"
                }
            );
            ensure!(
                values.is_some() != omission.is_some(),
                InvalidSnafu {
                    message: "ports must contain values or an omission"
                }
            );
            ensure!(
                identity.node.id.len() <= 1024
                    && !identity.node.id.is_empty()
                    && identity.node.kind.len() <= 256
                    && !identity.node.kind.is_empty()
                    && identity.node.path.len() <= crate::MAX_LOOP_DEPTH,
                InvalidSnafu {
                    message: "invalid node identity"
                }
            );
            ensure!(
                identity
                    .node
                    .path
                    .iter()
                    .all(|entry| entry.loop_id.len() <= 1024),
                InvalidSnafu {
                    message: "invalid Loop path"
                }
            );
            ensure!(
                matches!(
                    identity.stream.trigger,
                    StreamTrigger::Input | StreamTrigger::Message
                ) == identity.stream.message.is_some(),
                InvalidSnafu {
                    message: "trigger and input message disagree"
                }
            );
            ensure!(
                match direction {
                    Direction::Input => identity.ordinal.is_none() && identity.output.is_none(),
                    Direction::Output => identity.ordinal.is_some(),
                },
                InvalidSnafu {
                    message: "invalid emission identity"
                }
            );
            ensure!(
                identity
                    .output
                    .iter()
                    .chain(identity.stream.message.iter())
                    .all(|message| i64::try_from(message.domain).is_ok()),
                InvalidSnafu {
                    message: "message domain exceeds OTLP range"
                }
            );
            if let Some(value) = values {
                ensure!(
                    matches!(value, DataValue::Object(_)),
                    InvalidSnafu {
                        message: "port values must be an object"
                    }
                );
                value.validate(0)?;
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<EncodedDataRecord, DataError> {
        self.validate()?;
        let mut writer = LimitedWriter(Vec::new());
        let result = serde_json::to_writer(&mut writer, self);
        if writer.0.len() == MAX_RECORD_BYTES && result.is_err() {
            return TooLargeSnafu.fail();
        }
        result.context(JsonSnafu)?;
        Ok(EncodedDataRecord {
            bytes: writer.0.into(),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id,
            sequence: self.sequence,
            time_unix_nano: self.time_unix_nano,
            trace_context: self.trace_context.clone(),
        })
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DataError> {
        ensure!(bytes.len() <= MAX_RECORD_BYTES, TooLargeSnafu);
        let record: Self = serde_json::from_slice(bytes).context(JsonSnafu)?;
        record.validate()?;
        Ok(record)
    }
}

struct LimitedWriter(Vec<u8>);
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = bytes
            .len()
            .min(MAX_RECORD_BYTES.saturating_sub(self.0.len()));
        if count == 0 {
            return Err(io::Error::other("stream data byte budget"));
        }
        self.0.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct EncodedDataRecord {
    pub bytes: Arc<[u8]>,
    pub workflow_id: WorkflowId,
    pub run_id: RunId,
    pub sequence: u64,
    pub time_unix_nano: u64,
    pub trace_context: Option<TraceContext>,
}

impl EncodedDataRecord {
    pub fn retained_bytes(&self) -> usize {
        self.bytes.len() + 1024
    }

    pub fn write_to(
        &self,
        record: &mut impl opentelemetry::logs::LogRecord,
    ) -> Result<(), DataError> {
        use opentelemetry::{
            logs::AnyValue,
            trace::{SpanId, TraceFlags, TraceId},
        };
        record.set_event_name(RECORD_EVENT);
        record.set_timestamp(
            std::time::UNIX_EPOCH + std::time::Duration::from_nanos(self.time_unix_nano),
        );
        record.add_attribute("mf.workflow.id", self.workflow_id.to_string());
        record.add_attribute("mf.run.id", self.run_id.to_string());
        record.add_attribute("mf.stream.data.version", i64::from(SCHEMA_VERSION));
        record.add_attribute("mf.stream.data.sequence", self.sequence.to_string());
        record.set_body(AnyValue::Bytes(Box::new(self.bytes.to_vec())));
        if let Some(context) = &self.trace_context {
            let trace = TraceId::from_hex(&context.trace_id).context(crate::TraceIdSnafu)?;
            let span = SpanId::from_hex(&context.span_id).context(crate::SpanIdSnafu)?;
            record.set_trace_context(trace, span, Some(TraceFlags::new(context.trace_flags)));
        }
        Ok(())
    }
}

#[cfg(feature = "snapshot")]
pub fn from_log(
    record: &opentelemetry_proto::tonic::logs::v1::LogRecord,
) -> Result<DataRecord, DataError> {
    use opentelemetry_proto::tonic::common::v1::any_value;
    use prost::Message;
    ensure!(
        record.encoded_len() <= MAX_RECORD_BYTES + 4096,
        TooLargeSnafu
    );
    ensure!(
        record.attributes.len() <= 16
            && record
                .attributes
                .iter()
                .all(|attribute| attribute.key_strindex == 0),
        InvalidSnafu {
            message: "invalid data routing attributes"
        }
    );
    ensure!(
        record.event_name == RECORD_EVENT,
        InvalidSnafu {
            message: "unexpected data event"
        }
    );
    let Some(any_value::Value::BytesValue(bytes)) =
        record.body.as_ref().and_then(|body| body.value.as_ref())
    else {
        return InvalidSnafu {
            message: "data body must contain encoded bytes",
        }
        .fail();
    };
    let decoded = DataRecord::decode(bytes)?;
    ensure!(
        record.time_unix_nano == decoded.time_unix_nano,
        InvalidSnafu {
            message: "data timestamp disagrees with body"
        }
    );
    for key in [
        "mf.workflow.id",
        "mf.run.id",
        "mf.stream.data.sequence",
        "mf.stream.data.version",
    ] {
        ensure!(
            record
                .attributes
                .iter()
                .filter(|attribute| attribute.key == key)
                .count()
                == 1,
            InvalidSnafu {
                message: "missing or duplicate routing attribute"
            }
        );
    }
    let attr = |key: &str| {
        record
            .attributes
            .iter()
            .find(|attr| attr.key == key)
            .and_then(|attr| attr.value.as_ref())
            .and_then(|value| value.value.as_ref())
    };
    ensure!(
        attr("mf.workflow.id")
            == Some(&any_value::Value::StringValue(
                decoded.workflow_id.to_string()
            ))
            && attr("mf.run.id")
                == Some(&any_value::Value::StringValue(decoded.run_id.to_string()))
            && attr("mf.stream.data.sequence")
                == Some(&any_value::Value::StringValue(decoded.sequence.to_string()))
            && attr("mf.stream.data.version")
                == Some(&any_value::Value::IntValue(i64::from(decoded.version))),
        InvalidSnafu {
            message: "data envelope routing disagrees with attributes"
        }
    );
    if let Some(context) = &decoded.trace_context {
        ensure!(
            record.trace_id
                == opentelemetry::trace::TraceId::from_hex(&context.trace_id)
                    .context(crate::TraceIdSnafu)?
                    .to_bytes()
                && record.span_id
                    == opentelemetry::trace::SpanId::from_hex(&context.span_id)
                        .context(crate::SpanIdSnafu)?
                        .to_bytes()
                && record.flags & 0xff == u32::from(context.trace_flags),
            InvalidSnafu {
                message: "data native context disagrees with body"
            }
        );
    } else {
        ensure!(
            record.trace_id.iter().all(|byte| *byte == 0)
                && record.span_id.iter().all(|byte| *byte == 0),
            InvalidSnafu {
                message: "uncorrelated data carries native context"
            }
        );
    }
    Ok(decoded)
}

pub struct CaptureSequence {
    next: Option<u64>,
}
impl Default for CaptureSequence {
    fn default() -> Self {
        Self::new()
    }
}
impl CaptureSequence {
    pub fn new() -> Self {
        Self { next: Some(0) }
    }
    pub fn reserve(&mut self) -> Result<u64, DataError> {
        let Some(sequence) = self.next else {
            return ExhaustedSnafu.fail();
        };
        self.next = sequence.checked_add(1);
        Ok(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(value: DataValue) -> DataRecord {
        DataRecord {
            version: SCHEMA_VERSION,
            workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
            run_id: RunId::new(),
            sequence: 9,
            time_unix_nano: 10,
            trace_context: None,
            payload: DataPayload::Ports {
                identity: DataIdentity {
                    node: NodeIdentity {
                        id: "node".into(),
                        kind: "test".into(),
                        path: Vec::new(),
                    },
                    stream: StreamIdentity {
                        invocation: u64::MAX,
                        trigger: StreamTrigger::Timer,
                        message: None,
                        parent: None,
                    },
                    output: Some(StreamMessage {
                        domain: 1,
                        sequence: u64::MAX,
                    }),
                    ordinal: Some(2),
                }
                .into(),
                direction: Direction::Output,
                port_names: vec!["value".into()],
                ports_truncated: false,
                values: Some(DataValue::Object(BTreeMap::from([("value".into(), value)]))),
                omission: None,
            },
        }
    }
    #[test]
    fn independent_records_preserve_numeric_types_and_large_identities() {
        let original = record(DataValue::Array(vec![
            DataValue::Number(u64::MAX.into()),
            DataValue::Number(Number::from_f64(2.0).unwrap()),
            DataValue::Null,
            DataValue::Bool(true),
            DataValue::String("hello".into()),
        ]));
        let encoded = original.encode().unwrap();
        let decoded = DataRecord::decode(&encoded.bytes).unwrap();
        assert_eq!(decoded, original);
        let DataPayload::Ports {
            values: Some(values),
            ..
        } = decoded.payload
        else {
            panic!("missing values")
        };
        let value = values.to_json();
        assert_eq!(value["value"][0].as_u64(), Some(u64::MAX));
        assert!(value["value"][1].is_f64());
    }
    #[test]
    fn records_enforce_byte_depth_and_identity_budgets() {
        assert!(matches!(
            record(DataValue::String("x".repeat(MAX_RECORD_BYTES))).encode(),
            Err(DataError::TooLarge)
        ));
        let mut value = DataValue::Null;
        for _ in 0..MAX_VALUE_DEPTH - 1 {
            value = DataValue::Array(vec![value]);
        }
        let boundary = record(value.clone());
        assert_eq!(
            DataRecord::decode(&boundary.encode().unwrap().bytes).unwrap(),
            boundary
        );
        value = DataValue::Array(vec![value]);
        assert!(matches!(record(value).encode(), Err(DataError::TooDeep)));
        let mut invalid = record(DataValue::Null);
        invalid.version += 1;
        assert!(matches!(invalid.encode(), Err(DataError::Invalid { .. })));
        assert!(matches!(
            DataRecord::decode(b"not-json"),
            Err(DataError::Json { .. })
        ));
    }
    #[test]
    fn capture_sequences_never_wrap_or_reuse_ids() {
        let mut initial = CaptureSequence::default();
        assert_eq!(initial.reserve().unwrap(), 0);
        assert_eq!(initial.reserve().unwrap(), 1);
        let mut sequence = CaptureSequence {
            next: Some(u64::MAX),
        };
        assert_eq!(sequence.reserve().unwrap(), u64::MAX);
        assert!(matches!(sequence.reserve(), Err(DataError::Exhausted)));
        assert!(matches!(sequence.reserve(), Err(DataError::Exhausted)));
    }

    #[cfg(feature = "snapshot")]
    #[test]
    fn native_context_and_routing_must_agree_with_the_encoded_record() {
        use opentelemetry_proto::tonic::{
            common::v1::{AnyValue, KeyValue, any_value},
            logs::v1::LogRecord,
        };
        let mut original = record(DataValue::Null);
        original.trace_context = Some(TraceContext {
            trace_id: "01".repeat(16),
            span_id: "02".repeat(8),
            trace_flags: 1,
        });
        let encoded = original.encode().unwrap();
        let attr = |key: &str, value| KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            ..Default::default()
        };
        let mut log = LogRecord {
            event_name: RECORD_EVENT.into(),
            time_unix_nano: original.time_unix_nano,
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            flags: 1,
            body: Some(AnyValue {
                value: Some(any_value::Value::BytesValue(encoded.bytes.to_vec())),
            }),
            attributes: vec![
                attr(
                    "mf.workflow.id",
                    any_value::Value::StringValue(original.workflow_id.to_string()),
                ),
                attr(
                    "mf.run.id",
                    any_value::Value::StringValue(original.run_id.to_string()),
                ),
                attr(
                    "mf.stream.data.sequence",
                    any_value::Value::StringValue(original.sequence.to_string()),
                ),
                attr("mf.stream.data.version", any_value::Value::IntValue(1)),
            ],
            ..Default::default()
        };
        assert_eq!(from_log(&log).unwrap(), original);
        log.span_id = vec![3; 8];
        assert!(matches!(from_log(&log), Err(DataError::Invalid { .. })));
        log.span_id = vec![2; 8];
        log.attributes.push(log.attributes[0].clone());
        assert!(matches!(from_log(&log), Err(DataError::Invalid { .. })));
    }
}
