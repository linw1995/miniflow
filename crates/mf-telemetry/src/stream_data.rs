//! Independently decodable, bounded business data for streaming observations.

use crate::{
    event::LoopPathEntry,
    identity::{RunId, WorkflowId},
    stream::counter,
    wire::TraceContext,
};
use opentelemetry::trace::TraceContextExt;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
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

/// Borrowed value structure; capture policy belongs to the consumer.
pub enum ValueView<'a, V> {
    Null,
    Bool(bool),
    Number(&'a Number),
    String(&'a str),
    Array(&'a [V]),
    Object,
}

pub trait DataValueSource: Sized {
    fn view(&self) -> ValueView<'_, Self>;
    fn object_items(&self) -> impl Iterator<Item = (&str, &Self)>;
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
        node: String,
        scope: Vec<LoopPathEntry>,
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
            node,
            scope,
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
                !node.is_empty() && node.len() <= 1024 && scope.len() <= crate::MAX_LOOP_DEPTH,
                InvalidSnafu {
                    message: "invalid node scope"
                }
            );
            ensure!(
                scope.iter().all(|entry| entry.loop_id.len() <= 1024),
                InvalidSnafu {
                    message: "invalid Loop path"
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

#[derive(Debug, Snafu)]
pub enum StreamCaptureError {
    #[snafu(transparent)]
    Contract { source: DataError },
    #[snafu(display("could not encode stream data #{sequence}: {source}"))]
    Encode { sequence: u64, source: DataError },
    #[snafu(display("stream data sink failed for record #{sequence}: {source}"))]
    Sink {
        sequence: u64,
        source: Box<dyn Error + Send + Sync>,
    },
    #[snafu(display("stream data sink panicked"))]
    SinkPanicked,
}

type DataSink =
    dyn Fn(EncodedDataRecord) -> Result<bool, Box<dyn Error + Send + Sync>> + Send + Sync;
type Available = dyn Fn() -> bool + Send + Sync;
struct CaptureState {
    sequence: CaptureSequence,
    counts: CaptureCounts,
    closed: bool,
    error: Option<Arc<StreamCaptureError>>,
}
struct CaptureInner {
    observation: crate::observation::StreamObservation,
    state: Mutex<CaptureState>,
    sink: Box<DataSink>,
    available: Box<Available>,
}

#[derive(Clone)]
pub struct StreamDataRecorder(Arc<CaptureInner>);
impl fmt::Debug for StreamDataRecorder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamDataRecorder")
            .field("run_id", &self.0.observation.run_id())
            .finish_non_exhaustive()
    }
}

impl StreamDataRecorder {
    /// The sink and availability callback must be bounded and nonblocking.
    pub fn new(
        observation: crate::observation::StreamObservation,
        sink: impl Fn(EncodedDataRecord) -> Result<bool, Box<dyn Error + Send + Sync>>
        + Send
        + Sync
        + 'static,
        available: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        let recorder = Self(Arc::new(CaptureInner {
            observation,
            state: Mutex::new(CaptureState {
                sequence: CaptureSequence::new(),
                counts: CaptureCounts::default(),
                closed: false,
                error: None,
            }),
            sink: Box::new(sink),
            available: Box::new(available),
        }));
        recorder.control(false);
        recorder
    }

    pub fn diagnostic(&self) -> Option<Arc<StreamCaptureError>> {
        self.0.state.lock().unwrap().error.clone()
    }

    fn fail(&self, error: StreamCaptureError) {
        self.0
            .state
            .lock()
            .unwrap()
            .error
            .get_or_insert_with(|| Arc::new(error));
    }

    fn sequence(&self) -> Option<u64> {
        let mut state = self.0.state.lock().unwrap();
        if state.closed || state.error.is_some() {
            return None;
        }
        match state.sequence.reserve() {
            Ok(sequence) => Some(sequence),
            Err(source) => {
                state.error = Some(Arc::new(source.into()));
                None
            }
        }
    }

    pub fn record<V: DataValueSource>(
        &self,
        node: &str,
        scope: &[LoopPathEntry],
        direction: Direction,
        ports: &V,
    ) {
        let Some(sequence) = self.sequence() else {
            return;
        };
        let context = opentelemetry::Context::current();
        let span = context.span();
        let native = span.span_context();
        let trace_context = native.is_valid().then(|| TraceContext {
            trace_id: native.trace_id().to_string(),
            span_id: native.span_id().to_string(),
            trace_flags: native.trace_flags().to_u8(),
        });
        let time_unix_nano = timestamp();
        let result = catch_unwind(AssertUnwindSafe(|| {
            if !(self.0.available)() {
                self.increment(false)?;
                return Ok(());
            }
            let (port_names, ports_truncated) = port_summary(ports);
            let mut budget = MAX_RECORD_BYTES - 4096;
            let values = capture_value(ports, &mut budget, 0);
            let (values, omission) = match values {
                Ok(value) => (Some(value), None),
                Err(DataError::TooLarge) => (None, Some(Omission::Bytes)),
                Err(DataError::TooDeep) => (None, Some(Omission::Depth)),
                Err(error) => return Err(error.into()),
            };
            let mut record = self.envelope(
                sequence,
                time_unix_nano,
                trace_context,
                DataPayload::Ports {
                    node: node.into(),
                    scope: scope.to_vec(),
                    direction,
                    port_names,
                    ports_truncated,
                    values,
                    omission,
                },
            );
            let packet = match record.encode() {
                Ok(packet) => packet,
                Err(DataError::TooLarge) => {
                    let DataPayload::Ports {
                        values, omission, ..
                    } = &mut record.payload
                    else {
                        unreachable!()
                    };
                    *values = None;
                    *omission = Some(Omission::Bytes);
                    record.encode().context(EncodeSnafu { sequence })?
                }
                Err(source) => return Err(EncodeSnafu { sequence }.into_error(source)),
            };
            if matches!(
                record.payload,
                DataPayload::Ports {
                    omission: Some(_),
                    ..
                }
            ) {
                self.increment(true)?;
            }
            if !(self.0.sink)(packet).context(SinkSnafu { sequence })? {
                self.increment(false)?;
            }
            Ok(())
        }));
        match result {
            Ok(Ok(())) => {
                if sequence % 64 == 63 {
                    self.control(false);
                }
            }
            Ok(Err(error)) => self.fail(error),
            Err(_) => self.fail(SinkPanickedSnafu.build()),
        }
    }

    fn increment(&self, omitted: bool) -> Result<(), StreamCaptureError> {
        let mut state = self.0.state.lock().unwrap();
        let count = if omitted {
            &mut state.counts.omitted
        } else {
            &mut state.counts.dropped
        };
        *count = count.checked_add(1).context(ExhaustedSnafu)?;
        Ok(())
    }

    fn envelope(
        &self,
        sequence: u64,
        time_unix_nano: u64,
        trace_context: Option<TraceContext>,
        payload: DataPayload,
    ) -> DataRecord {
        DataRecord {
            version: SCHEMA_VERSION,
            workflow_id: self.0.observation.description().workflow_id.clone(),
            run_id: self.0.observation.run_id(),
            sequence,
            time_unix_nano,
            trace_context,
            payload,
        }
    }

    fn control(&self, end: bool) {
        let (sequence, payload) = {
            let mut state = self.0.state.lock().unwrap();
            if state.closed || state.error.is_some() {
                return;
            }
            let sequence = match state.sequence.reserve() {
                Ok(sequence) => sequence,
                Err(source) => {
                    state.error = Some(Arc::new(source.into()));
                    return;
                }
            };
            let payload = if end {
                state.closed = true;
                DataPayload::End {
                    counts: state.counts,
                }
            } else if sequence == 0 {
                DataPayload::Start
            } else {
                DataPayload::Status {
                    counts: state.counts,
                }
            };
            (sequence, payload)
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let packet = self
                .envelope(sequence, timestamp(), None, payload)
                .encode()
                .context(EncodeSnafu { sequence })?;
            if !(self.0.sink)(packet).context(SinkSnafu { sequence })? {
                self.increment(false)?;
            }
            Ok::<_, StreamCaptureError>(())
        }));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.fail(error),
            Err(_) => self.fail(SinkPanickedSnafu.build()),
        }
    }

    pub fn finish(&self) {
        self.control(true);
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn port_summary<V: DataValueSource>(ports: &V) -> (Vec<String>, bool) {
    let mut truncated = ports.object_items().take(33).count() > 32;
    let names = ports
        .object_items()
        .take(32)
        .map(|(name, _)| {
            if name.len() <= 256 {
                return name.to_owned();
            }
            truncated = true;
            let mut end = 253;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &name[..end])
        })
        .collect();
    (names, truncated)
}

fn spend(budget: &mut usize, cost: usize) -> Result<(), DataError> {
    *budget = budget.checked_sub(cost).context(TooLargeSnafu)?;
    Ok(())
}

fn text_cost(text: &str, budget: &mut usize) -> Result<(), DataError> {
    ensure!(text.len() <= *budget, TooLargeSnafu);
    spend(budget, 2)?;
    for byte in text.bytes() {
        spend(
            budget,
            match byte {
                b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
                0..=31 => 6,
                _ => 1,
            },
        )?;
    }
    Ok(())
}

fn capture_value<V: DataValueSource>(
    value: &V,
    budget: &mut usize,
    depth: usize,
) -> Result<DataValue, DataError> {
    ensure!(depth <= MAX_VALUE_DEPTH, TooDeepSnafu);
    spend(budget, 64)?;
    Ok(match value.view() {
        ValueView::Null => DataValue::Null,
        ValueView::Bool(value) => DataValue::Bool(value),
        ValueView::Number(value) => DataValue::Number(value.clone()),
        ValueView::String(value) => {
            text_cost(value, budget)?;
            DataValue::String(value.to_string())
        }
        ValueView::Array(items) => {
            let mut values = Vec::new();
            for item in items {
                values.push(capture_value(item, budget, depth + 1)?);
            }
            DataValue::Array(values)
        }
        ValueView::Object => {
            let mut values = BTreeMap::new();
            for (key, item) in value.object_items() {
                text_cost(key, budget)?;
                values.insert(key.to_string(), capture_value(item, budget, depth + 1)?);
            }
            DataValue::Object(values)
        }
    })
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
                node: "node".into(),
                scope: Vec::new(),
                direction: Direction::Output,
                port_names: vec!["value".into()],
                ports_truncated: false,
                values: Some(DataValue::Object(BTreeMap::from([("value".into(), value)]))),
                omission: None,
            },
        }
    }
    #[test]
    fn independent_records_preserve_numeric_types() {
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
    fn records_enforce_byte_and_depth_budgets() {
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
