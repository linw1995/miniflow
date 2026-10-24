use crate::{
    LiveOutputEvent, LiveOutputKind, LivePublisher, NodeValues, StreamCancellation, StreamError,
    StreamInvocation, StreamOutputContract,
};
use mf_telemetry::identity::RunId;
use serde::{Deserialize, Serialize};
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, TryLockError},
    time::Duration,
};

pub const LIVE_PROTOCOL_VERSION: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRecord {
    pub version: u32,
    pub run_id: String,
    pub sequence: u64,
    pub record: LiveRecordKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRecordKind {
    Event { event: LiveOutputEvent },
    Final { outputs: NodeValues },
}

#[derive(Clone, Debug, Snafu)]
pub enum LiveProtocolError {
    #[snafu(display("unsupported live protocol version {version}"))]
    Version { version: u32 },
    #[snafu(display("live output run identity does not match the launched execution"))]
    RunIdentity,
    #[snafu(display("live output sequence {actual} does not match expected {expected}"))]
    Sequence { expected: u64, actual: u64 },
    #[snafu(display("live output sequence counter exhausted"))]
    Counter,
    #[snafu(display("live output names an undeclared export"))]
    UnknownExport { name: String },
    #[snafu(display("live output source for `{name}` differs from its declared contract"))]
    Source { name: String },
    #[snafu(display("live output `{name}` invocation {id}: {reason}"))]
    Lifecycle {
        name: String,
        id: u64,
        reason: &'static str,
    },
    #[snafu(display("live output contains records after its final result"))]
    AfterFinal,
    #[snafu(display("live output final result arrived before all streams closed"))]
    ActiveStreams,
    #[snafu(display("live output ended without a final result"))]
    MissingFinal,
    #[snafu(display("live output protocol already failed"))]
    Failed,
    #[snafu(display("live output writer is unavailable"))]
    WriterUnavailable,
    #[snafu(display("invalid live output record"))]
    Json {
        #[snafu(source(from(serde_json::Error, Arc::new)))]
        source: Arc<serde_json::Error>,
    },
    #[snafu(display("invalid live output contract: {source}"))]
    Contract {
        #[snafu(source(from(crate::WorkflowInputError, Arc::new)))]
        source: Arc<crate::WorkflowInputError>,
    },
    #[snafu(display("live output `{name}` violates its value contract: {source}"))]
    ValueType {
        name: String,
        #[snafu(source(from(crate::TypeMismatch, Arc::new)))]
        source: Arc<crate::TypeMismatch>,
    },
    #[snafu(display("could not deliver live output: {source}"), visibility(pub))]
    Delivery {
        #[snafu(source(from(StreamError, Arc::new)))]
        source: Arc<StreamError>,
    },
}

impl From<LiveProtocolError> for StreamError {
    fn from(error: LiveProtocolError) -> Self {
        crate::stream_instance::LiveProtocolSnafu.into_error(error)
    }
}

struct ProtocolState {
    run_id: String,
    next: u64,
    contracts: BTreeMap<String, StreamOutputContract>,
    active: BTreeMap<(String, u64), StreamInvocation>,
    seen: BTreeSet<(String, u64)>,
    finalized: bool,
}

impl ProtocolState {
    fn new(run_id: RunId, contracts: &[StreamOutputContract]) -> Result<Self, LiveProtocolError> {
        crate::workflow_inputs::validate_stream_outputs(contracts).context(ContractSnafu)?;
        Ok(Self {
            run_id: run_id.to_string(),
            next: 0,
            contracts: contracts
                .iter()
                .map(|contract| (contract.name.clone(), contract.clone()))
                .collect(),
            active: BTreeMap::new(),
            seen: BTreeSet::new(),
            finalized: false,
        })
    }

    fn accept(&mut self, record: &LiveRecord) -> Result<(), LiveProtocolError> {
        ensure!(!self.finalized, AfterFinalSnafu);
        ensure!(
            record.version == LIVE_PROTOCOL_VERSION,
            VersionSnafu {
                version: record.version
            }
        );
        ensure!(record.run_id == self.run_id, RunIdentitySnafu);
        ensure!(
            record.sequence == self.next,
            SequenceSnafu {
                expected: self.next,
                actual: record.sequence
            }
        );
        let next = self.next.checked_add(1).context(CounterSnafu)?;
        match &record.record {
            LiveRecordKind::Event { event } => {
                let contract = self
                    .contracts
                    .get(&event.export)
                    .context(UnknownExportSnafu {
                        name: &event.export,
                    })?;
                ensure!(
                    event.source == contract.path
                        && contract
                            .path
                            .last()
                            .is_some_and(|node| node.as_str() == event.invocation.node)
                        && event
                            .invocation
                            .path
                            .iter()
                            .all(|scope| !scope.loop_id.trim().is_empty()),
                    SourceSnafu {
                        name: &event.export
                    }
                );
                let key = (event.export.clone(), event.invocation.id);
                let invalid = |reason| {
                    LifecycleSnafu {
                        name: &event.export,
                        id: event.invocation.id,
                        reason,
                    }
                    .build()
                };
                match &event.event {
                    LiveOutputKind::Open => {
                        ensure!(
                            self.seen.insert(key.clone()),
                            LifecycleSnafu {
                                name: &event.export,
                                id: event.invocation.id,
                                reason: "invocation was already opened"
                            }
                        );
                        self.active.insert(key, event.invocation.clone());
                    }
                    LiveOutputKind::Value { value } => {
                        let identity = self
                            .active
                            .get(&key)
                            .ok_or_else(|| invalid("value arrived outside an open invocation"))?;
                        ensure!(
                            identity == &event.invocation,
                            LifecycleSnafu {
                                name: &event.export,
                                id: event.invocation.id,
                                reason: "invocation identity changed"
                            }
                        );
                        contract
                            .value_type
                            .validate_shared(value)
                            .context(ValueTypeSnafu {
                                name: &event.export,
                            })?;
                    }
                    LiveOutputKind::Close { .. } => {
                        let identity = self
                            .active
                            .remove(&key)
                            .ok_or_else(|| invalid("close arrived outside an open invocation"))?;
                        ensure!(
                            identity == event.invocation,
                            LifecycleSnafu {
                                name: &event.export,
                                id: event.invocation.id,
                                reason: "invocation identity changed"
                            }
                        );
                    }
                }
            }
            LiveRecordKind::Final { .. } => {
                ensure!(self.active.is_empty(), ActiveStreamsSnafu);
                self.finalized = true;
            }
        }
        self.next = next;
        Ok(())
    }
}

type WriteRecord = dyn FnMut(&[u8], &StreamCancellation) -> Result<(), StreamError> + Send;

struct WriterState {
    protocol: ProtocolState,
    write: Box<WriteRecord>,
    failure: Option<LiveProtocolError>,
}

#[derive(Clone)]
pub struct LiveOutputWriter(Arc<Mutex<WriterState>>);

impl LiveOutputWriter {
    pub fn new(
        run_id: RunId,
        contracts: &[StreamOutputContract],
        write: impl FnMut(&[u8], &StreamCancellation) -> Result<(), StreamError> + Send + 'static,
    ) -> Result<Self, LiveProtocolError> {
        Ok(Self(Arc::new(Mutex::new(WriterState {
            protocol: ProtocolState::new(run_id, contracts)?,
            write: Box::new(write),
            failure: None,
        }))))
    }

    pub fn publisher(&self) -> LivePublisher {
        let writer = self.clone();
        LivePublisher::new(move |event, cancellation| {
            writer
                .record(LiveRecordKind::Event { event }, cancellation)
                .map_err(StreamError::from)
        })
    }

    pub fn finish(
        &self,
        outputs: &NodeValues,
        cancellation: &StreamCancellation,
    ) -> Result<(), LiveProtocolError> {
        self.record(
            LiveRecordKind::Final {
                outputs: outputs.clone(),
            },
            cancellation,
        )
    }

    fn record(
        &self,
        payload: LiveRecordKind,
        cancellation: &StreamCancellation,
    ) -> Result<(), LiveProtocolError> {
        let mut state = loop {
            if let Some(error) = cancellation.failure() {
                return Err(error).context(DeliverySnafu);
            }
            match self.0.try_lock() {
                Ok(state) => break state,
                Err(TryLockError::Poisoned(_)) => return WriterUnavailableSnafu.fail(),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let result = state.write(payload, cancellation);
        if let Err(error) = &result {
            state.failure = Some(error.clone());
        }
        result
    }
}

impl WriterState {
    fn write(
        &mut self,
        record: LiveRecordKind,
        cancellation: &StreamCancellation,
    ) -> Result<(), LiveProtocolError> {
        let packet = LiveRecord {
            version: LIVE_PROTOCOL_VERSION,
            run_id: self.protocol.run_id.clone(),
            sequence: self.protocol.next,
            record,
        };
        self.protocol.accept(&packet)?;
        let mut bytes = serde_json::to_vec(&packet).context(JsonSnafu)?;
        bytes.push(b'\n');
        (self.write)(&bytes, cancellation).context(DeliverySnafu)?;
        Ok(())
    }
}

pub struct LiveOutputDecoder {
    protocol: ProtocolState,
    partial: Vec<u8>,
    final_outputs: Option<NodeValues>,
    failed: bool,
}

impl LiveOutputDecoder {
    pub fn new(
        run_id: RunId,
        contracts: &[StreamOutputContract],
    ) -> Result<Self, LiveProtocolError> {
        Ok(Self {
            protocol: ProtocolState::new(run_id, contracts)?,
            partial: Vec::new(),
            final_outputs: None,
            failed: false,
        })
    }

    pub fn final_outputs(&self) -> Option<&NodeValues> {
        self.final_outputs.as_ref()
    }

    pub fn take_final_outputs(&mut self) -> Option<NodeValues> {
        let outputs = self.final_outputs.take();
        if outputs.is_some() && self.partial.is_empty() {
            self.partial.shrink_to_fit();
        }
        outputs
    }

    pub fn feed(
        &mut self,
        bytes: &[u8],
        mut emit: impl FnMut(LiveOutputEvent) -> Result<(), StreamError>,
    ) -> Result<(), LiveProtocolError> {
        ensure!(!self.failed, FailedSnafu);
        let result = self.feed_inner(bytes, &mut emit);
        self.failed |= result.is_err();
        result
    }

    fn feed_inner(
        &mut self,
        bytes: &[u8],
        emit: &mut dyn FnMut(LiveOutputEvent) -> Result<(), StreamError>,
    ) -> Result<(), LiveProtocolError> {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.partial.extend_from_slice(part);
            if part.last() == Some(&b'\n') {
                let mut record = std::mem::take(&mut self.partial);
                self.decode(&record, emit)?;
                record.clear();
                self.partial = record;
            }
        }
        Ok(())
    }

    fn decode(
        &mut self,
        bytes: &[u8],
        emit: &mut dyn FnMut(LiveOutputEvent) -> Result<(), StreamError>,
    ) -> Result<(), LiveProtocolError> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let record: LiveRecord = crate::decode_unique_json(bytes).context(JsonSnafu)?;
        self.protocol.accept(&record)?;
        match record.record {
            LiveRecordKind::Event { event } => emit(event).context(DeliverySnafu),
            LiveRecordKind::Final { outputs } => {
                self.final_outputs = Some(outputs);
                Ok(())
            }
        }
    }

    pub fn finish(
        &mut self,
        mut emit: impl FnMut(LiveOutputEvent) -> Result<(), StreamError>,
    ) -> Result<(), LiveProtocolError> {
        ensure!(!self.failed, FailedSnafu);
        let partial = std::mem::take(&mut self.partial);
        let result = self.decode(&partial, &mut emit).and_then(|()| {
            ensure!(self.protocol.finalized, MissingFinalSnafu);
            Ok(())
        });
        self.failed |= result.is_err();
        result
    }
}
