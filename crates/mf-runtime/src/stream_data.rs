//! Opt-in capture without retaining previous stream values.

use crate::{Inputs, StreamObservation, ValueKind, ValueRef};
use mf_telemetry::{
    stream::StreamMessage,
    stream_data::{
        CaptureCounts, CaptureSequence, DataError, DataIdentity, DataPayload, DataRecord,
        DataValue, Direction, EncodedDataRecord, MAX_RECORD_BYTES, MAX_VALUE_DEPTH, Omission,
        SCHEMA_VERSION, TooDeepSnafu, TooLargeSnafu,
    },
    wire::TraceContext,
};
use opentelemetry::trace::TraceContextExt;
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

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
    #[snafu(display("stream data capture has no matching node observation"))]
    MissingIdentity,
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
    observation: StreamObservation,
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

pub struct StreamDataReservation {
    sequence: u64,
    identity: DataIdentity,
    direction: Direction,
    trace_context: Option<TraceContext>,
    time_unix_nano: u64,
}

impl StreamDataRecorder {
    /// The sink and availability callback must be bounded and nonblocking.
    pub fn new(
        observation: StreamObservation,
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

    pub fn matches(&self, observation: &StreamObservation) -> bool {
        self.0.observation.is_same_run(observation) && !self.0.state.lock().unwrap().closed
    }

    pub fn is_active(&self) -> bool {
        let state = self.0.state.lock().unwrap();
        !state.closed && state.error.is_none()
    }

    pub fn diagnostic(&self) -> Option<Arc<StreamCaptureError>> {
        self.0.state.lock().unwrap().error.clone()
    }

    pub fn missing_identity(&self) {
        self.fail(MissingIdentitySnafu.build());
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

    pub fn reserve(
        &self,
        mut identity: DataIdentity,
        direction: Direction,
        output: Option<StreamMessage>,
        ordinal: Option<u64>,
    ) -> Option<StreamDataReservation> {
        let sequence = self.sequence()?;
        identity.output = output;
        identity.ordinal = ordinal;
        let context = opentelemetry::Context::current();
        let span = context.span();
        let native = span.span_context();
        let trace_context = native.is_valid().then(|| TraceContext {
            trace_id: native.trace_id().to_string(),
            span_id: native.span_id().to_string(),
            trace_flags: native.trace_flags().to_u8(),
        });
        Some(StreamDataReservation {
            sequence,
            identity,
            direction,
            trace_context,
            time_unix_nano: timestamp(),
        })
    }

    pub fn inputs(&self, identity: DataIdentity, ports: &Inputs) {
        if let Some(reservation) = self.reserve(identity, Direction::Input, None, None) {
            self.publish(reservation, ports);
        }
    }

    pub fn outputs(&self, identity: DataIdentity, ports: &Inputs) {
        let output = identity.stream.message;
        if let Some(reservation) = self.reserve(identity, Direction::Output, output, Some(0)) {
            self.publish(reservation, ports);
        }
    }

    pub fn publish(&self, reservation: StreamDataReservation, ports: &Inputs) {
        let sequence = reservation.sequence;
        let result = catch_unwind(AssertUnwindSafe(|| {
            if !(self.0.available)() {
                self.increment(false)?;
                return Ok(());
            }
            let (port_names, ports_truncated) = port_summary(ports);
            let mut budget = MAX_RECORD_BYTES - 4096;
            let values = capture_ports(ports, &mut budget);
            let (values, omission) = match values {
                Ok(value) => (Some(value), None),
                Err(DataError::TooLarge) => (None, Some(Omission::Bytes)),
                Err(DataError::TooDeep) => (None, Some(Omission::Depth)),
                Err(error) => return Err(error.into()),
            };
            let mut record = self.record(
                sequence,
                reservation.time_unix_nano,
                reservation.trace_context,
                DataPayload::Ports {
                    identity: reservation.identity.into(),
                    direction: reservation.direction,
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
        *count = count
            .checked_add(1)
            .context(mf_telemetry::stream_data::ExhaustedSnafu)?;
        Ok(())
    }

    fn record(
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
                .record(sequence, timestamp(), None, payload)
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

fn port_summary(ports: &Inputs) -> (Vec<String>, bool) {
    let mut truncated = ports.len() > 32;
    let names = ports
        .keys()
        .take(32)
        .map(|name| {
            if name.len() <= 256 {
                return name.clone();
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

fn capture_ports(ports: &Inputs, budget: &mut usize) -> Result<DataValue, DataError> {
    spend(budget, 64)?;
    let mut values = BTreeMap::new();
    for (key, value) in ports {
        text_cost(key, budget)?;
        values.insert(key.clone(), capture_value(value, budget, 1)?);
    }
    Ok(DataValue::Object(values))
}

fn capture_value(
    value: &ValueRef,
    budget: &mut usize,
    depth: usize,
) -> Result<DataValue, DataError> {
    ensure!(depth <= MAX_VALUE_DEPTH, TooDeepSnafu);
    spend(budget, 64)?;
    Ok(match value.kind() {
        ValueKind::Null => DataValue::Null,
        ValueKind::Bool(value) => DataValue::Bool(*value),
        ValueKind::Number(value) => DataValue::Number(value.clone()),
        ValueKind::String(value) => {
            text_cost(value, budget)?;
            DataValue::String(value.to_string())
        }
        ValueKind::Array(items) => {
            let mut values = Vec::new();
            for item in items {
                values.push(capture_value(item, budget, depth + 1)?);
            }
            DataValue::Array(values)
        }
        ValueKind::Object(items) => {
            let mut values = BTreeMap::new();
            for (key, item) in items {
                text_cost(key, budget)?;
                values.insert(key.to_string(), capture_value(item, budget, depth + 1)?);
            }
            DataValue::Object(values)
        }
    })
}
