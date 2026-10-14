//! Caller-owned OTLP/HTTP providers for a standalone workflow process.

use crate::observation::Observer;
use opentelemetry_otlp::{LogExporter, Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    logs::{BatchConfigBuilder as LogBatchConfigBuilder, BatchLogProcessor, SdkLoggerProvider},
    trace::{BatchConfigBuilder as SpanBatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};
use snafu::{IntoError, OptionExt, ResultExt, Snafu, ensure};
use std::{env, sync::Arc, time::Duration};

pub const INTERACTIVE_BATCH_DELAY: Duration = Duration::from_millis(100);
pub const EXPORT_TIMEOUT: Duration = Duration::from_secs(2);
pub const MAX_QUEUE_SIZE: usize = 1024;
pub const MAX_BATCH_SIZE: usize = 128;
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub enum SetupError {
    #[snafu(display("could not create the blocking OTLP HTTP client: {source}"))]
    HttpClient { source: reqwest::Error },
    #[snafu(display("could not configure OTLP traces: {source}"))]
    Traces {
        source: opentelemetry_otlp::ExporterBuildError,
    },
    #[snafu(display("could not configure OTLP logs: {source}"))]
    Logs {
        source: opentelemetry_otlp::ExporterBuildError,
    },
}

#[derive(Debug, Snafu)]
pub enum ShutdownError {
    #[snafu(display("OTLP logs: {source}"))]
    LogsShutdown {
        source: opentelemetry_sdk::error::OTelSdkError,
    },
    #[snafu(display("OTLP traces: {source}"))]
    TracesShutdown {
        source: opentelemetry_sdk::error::OTelSdkError,
    },
}

pub struct TelemetryProviders {
    traces: SdkTracerProvider,
    logs: SdkLoggerProvider,
}

impl TelemetryProviders {
    /// Returns `None` before constructing SDK providers when no endpoint is configured.
    pub fn from_env() -> Result<Option<Self>, SetupError> {
        let common = configured("OTEL_EXPORTER_OTLP_ENDPOINT");
        let traces_enabled = common || configured("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT");
        let logs_enabled = common || configured("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT");
        if !traces_enabled && !logs_enabled {
            return Ok(None);
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(EXPORT_TIMEOUT)
            .build()
            .context(HttpClientSnafu)?;
        let traces_exporter = traces_enabled
            .then(|| {
                SpanExporter::builder()
                    .with_http()
                    .with_protocol(Protocol::HttpBinary)
                    .with_timeout(EXPORT_TIMEOUT)
                    .with_http_client(client.clone())
                    .with_max_request_body_size(MAX_REQUEST_BYTES)
                    .build()
                    .context(TracesSnafu)
            })
            .transpose()?;
        let logs_exporter = logs_enabled
            .then(|| {
                LogExporter::builder()
                    .with_http()
                    .with_protocol(Protocol::HttpBinary)
                    .with_timeout(EXPORT_TIMEOUT)
                    .with_http_client(client)
                    .with_max_request_body_size(MAX_REQUEST_BYTES)
                    .build()
                    .context(LogsSnafu)
            })
            .transpose()?;

        let resource = Resource::builder()
            .with_service_name("mf-generated-workflow")
            .build();
        let mut traces = SdkTracerProvider::builder().with_resource(resource.clone());
        if let Some(exporter) = traces_exporter {
            let config = SpanBatchConfigBuilder::default()
                .with_max_queue_size(MAX_QUEUE_SIZE)
                .with_max_export_batch_size(MAX_BATCH_SIZE)
                .with_scheduled_delay(INTERACTIVE_BATCH_DELAY)
                .build();
            traces = traces.with_span_processor(
                BatchSpanProcessor::builder(exporter)
                    .with_batch_config(config)
                    .build(),
            );
        }
        let mut logs = SdkLoggerProvider::builder().with_resource(resource);
        if let Some(exporter) = logs_exporter {
            let config = LogBatchConfigBuilder::default()
                .with_max_queue_size(MAX_QUEUE_SIZE)
                .with_max_export_batch_size(MAX_BATCH_SIZE)
                .with_scheduled_delay(INTERACTIVE_BATCH_DELAY)
                .build();
            logs = logs.with_log_processor(
                BatchLogProcessor::builder(exporter)
                    .with_batch_config(config)
                    .build(),
            );
        }
        Ok(Some(Self {
            traces: traces.build(),
            logs: logs.build(),
        }))
    }

    pub fn observer(&self) -> Observer {
        self.observer_with_verbosity(0)
    }

    pub fn observer_with_verbosity(&self, verbose: u8) -> Observer {
        Observer::new_with_verbosity(&self.traces, &self.logs, verbose)
    }

    /// Ends both pipelines within separate finite deadlines after a handled run.
    pub fn shutdown(self) -> Vec<ShutdownError> {
        let mut diagnostics = Vec::new();
        if let Err(error) = self.logs.shutdown_with_timeout(EXPORT_TIMEOUT) {
            diagnostics.push(LogsShutdownSnafu.into_error(error));
        }
        if let Err(error) = self.traces.shutdown_with_timeout(EXPORT_TIMEOUT) {
            diagnostics.push(TracesShutdownSnafu.into_error(error));
        }
        diagnostics
    }
}

fn configured(key: &str) -> bool {
    env::var(key).is_ok_and(|value| !value.trim().is_empty())
}

#[derive(Debug, Snafu)]
pub enum SnapshotExportError {
    #[snafu(transparent)]
    Setup { source: SetupError },
    #[snafu(transparent)]
    Packet {
        source: crate::snapshot::SnapshotError,
    },
    #[snafu(display("snapshot capture requires an OTLP logs endpoint"))]
    MissingEndpoint,
    #[snafu(display("snapshot exporter is closed"))]
    Closed,
    #[snafu(display("snapshot sequence overflow"))]
    SequenceOverflow,
    #[snafu(display("snapshot export failed: {source}"))]
    Delivery {
        source: Arc<opentelemetry_sdk::error::OTelSdkError>,
    },
    #[snafu(display("could not flush snapshot logs: {source}"))]
    Flush {
        source: opentelemetry_sdk::error::OTelSdkError,
    },
    #[snafu(display("could not shut down snapshot logs: {source}"))]
    Shutdown {
        source: opentelemetry_sdk::error::OTelSdkError,
    },
}

const SNAPSHOT_BATCH_SIZE: usize = 64;

#[derive(Debug)]
struct SnapshotLogExporter<E> {
    inner: E,
    error: std::sync::Arc<std::sync::Mutex<Option<Arc<opentelemetry_sdk::error::OTelSdkError>>>>,
}

impl<E: opentelemetry_sdk::logs::LogExporter> opentelemetry_sdk::logs::LogExporter
    for SnapshotLogExporter<E>
{
    async fn export(
        &self,
        batch: opentelemetry_sdk::logs::LogBatch<'_>,
    ) -> opentelemetry_sdk::error::OTelSdkResult {
        let result = self.inner.export(batch).await;
        if let Err(error) = &result {
            use opentelemetry_sdk::error::OTelSdkError;
            // The SDK error is not Clone; retain an equivalent typed failure for later calls.
            let cached = match error {
                OTelSdkError::AlreadyShutdown => OTelSdkError::AlreadyShutdown,
                OTelSdkError::Timeout(duration) => OTelSdkError::Timeout(*duration),
                OTelSdkError::InternalFailure(message) => {
                    OTelSdkError::InternalFailure(message.clone())
                }
            };
            self.error
                .lock()
                .expect("snapshot exporter was not poisoned")
                .get_or_insert_with(|| Arc::new(cached));
        }
        result
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

/// Snapshot records use the OTLP logs endpoint with bounded producer backpressure.
pub struct SnapshotExporter {
    logs: SdkLoggerProvider,
    logger: opentelemetry_sdk::logs::SdkLogger,
    workflow: crate::identity::WorkflowId,
    run: String,
    sequence: usize,
    buffered: usize,
    error: std::sync::Arc<std::sync::Mutex<Option<Arc<opentelemetry_sdk::error::OTelSdkError>>>>,
    closed: bool,
}

impl SnapshotExporter {
    pub fn from_env(
        workflow: crate::identity::WorkflowId,
        run: crate::identity::RunId,
    ) -> Result<Self, SnapshotExportError> {
        ensure!(
            configured("OTEL_EXPORTER_OTLP_ENDPOINT")
                || configured("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"),
            MissingEndpointSnafu
        );
        let client = reqwest::blocking::Client::builder()
            .timeout(EXPORT_TIMEOUT)
            .build()
            .context(HttpClientSnafu)?;
        let inner = LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(EXPORT_TIMEOUT)
            .with_http_client(client)
            .with_max_request_body_size(MAX_REQUEST_BYTES)
            .build()
            .context(LogsSnafu)?;
        Ok(Self::with_exporter(workflow, run, inner))
    }

    fn with_exporter<E: opentelemetry_sdk::logs::LogExporter + 'static>(
        workflow: crate::identity::WorkflowId,
        run: crate::identity::RunId,
        inner: E,
    ) -> Self {
        use opentelemetry::logs::LoggerProvider as _;
        let error = std::sync::Arc::new(std::sync::Mutex::new(None));
        let config = LogBatchConfigBuilder::default()
            .with_max_queue_size(SNAPSHOT_BATCH_SIZE * 2)
            .with_max_export_batch_size(SNAPSHOT_BATCH_SIZE)
            .with_scheduled_delay(INTERACTIVE_BATCH_DELAY)
            .build();
        let logs = SdkLoggerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name("mf-generated-workflow")
                    .build(),
            )
            .with_log_processor(
                BatchLogProcessor::builder(SnapshotLogExporter {
                    inner,
                    error: error.clone(),
                })
                .with_batch_config(config)
                .build(),
            )
            .build();
        let logger = logs.logger(crate::snapshot::SCOPE);
        Self {
            logs,
            logger,
            workflow,
            run: run.to_string(),
            sequence: 0,
            buffered: 0,
            error,
            closed: false,
        }
    }

    pub fn emit(&mut self, body: serde_json::Value) -> Result<(), SnapshotExportError> {
        use opentelemetry::logs::Logger as _;
        self.check_error()?;
        ensure!(!self.closed, ClosedSnafu);
        for packet in crate::snapshot::packets(self.sequence, body)? {
            self.check_error()?;
            let mut record = self.logger.create_log_record();
            packet.write_to(&mut record, self.workflow.as_str(), self.run.as_str())?;
            self.logger.emit(record);
            self.buffered += 1;
            // Waiting before filling the queue prevents a dropped value definition from
            // invalidating every later reference to that value.
            if self.buffered == SNAPSHOT_BATCH_SIZE {
                self.flush()?;
            }
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .context(SequenceOverflowSnafu)?;
        Ok(())
    }

    fn check_error(&self) -> Result<(), SnapshotExportError> {
        self.error
            .lock()
            .expect("snapshot exporter was not poisoned")
            .clone()
            .map_or(Ok(()), Err)
            .context(DeliverySnafu)
    }

    pub fn flush(&mut self) -> Result<(), SnapshotExportError> {
        let result = self.logs.force_flush().context(FlushSnafu);
        // Prefer the exporter failure over the SDK's flush summary of that failure.
        self.check_error()?;
        result?;
        self.buffered = 0;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), SnapshotExportError> {
        if self.closed {
            return self.check_error();
        }
        let result = self.flush();
        self.closed = true;
        let shutdown = self
            .logs
            .shutdown_with_timeout(EXPORT_TIMEOUT)
            .context(ShutdownSnafu);
        result.and(shutdown).and_then(|_| self.check_error())
    }
}

impl Drop for SnapshotExporter {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.logs.shutdown_with_timeout(EXPORT_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use opentelemetry::logs::AnyValue;
    use opentelemetry_sdk::{
        error::{OTelSdkError, OTelSdkResult},
        logs::{LogBatch, LogExporter},
    };
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct Collect(Arc<Mutex<Vec<i64>>>);
    impl LogExporter for Collect {
        async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
            let mut values = self.0.lock().unwrap();
            for (record, scope) in batch.iter() {
                assert_eq!(scope.name(), crate::snapshot::SCOPE);
                let (_, AnyValue::Int(sequence)) = record
                    .attributes_iter()
                    .find(|(key, _)| key.as_str() == "mf.snapshot.sequence")
                    .unwrap()
                else {
                    panic!("missing sequence")
                };
                values.push(*sequence);
            }
            Ok(())
        }
    }
    #[derive(Debug)]
    struct Fail;
    impl LogExporter for Fail {
        async fn export(&self, _: LogBatch<'_>) -> OTelSdkResult {
            Err(OTelSdkError::InternalFailure("delivery failed".into()))
        }
    }
    fn workflow() -> crate::identity::WorkflowId {
        crate::identity::WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap()
    }

    #[test]
    fn snapshot_backpressure_retains_bursts_larger_than_the_queue() {
        let collected = Arc::new(Mutex::new(Vec::new()));
        let mut exporter = SnapshotExporter::with_exporter(
            workflow(),
            crate::identity::RunId::new(),
            Collect(collected.clone()),
        );
        for index in 0..2000 {
            exporter.emit(serde_json::json!({"index": index})).unwrap();
        }
        exporter.finish().unwrap();
        assert_eq!(*collected.lock().unwrap(), (0..2000).collect::<Vec<_>>());
    }

    #[test]
    fn snapshot_delivery_errors_remain_visible_after_flush() {
        let mut exporter =
            SnapshotExporter::with_exporter(workflow(), crate::identity::RunId::new(), Fail);
        exporter
            .emit(serde_json::json!({"record":"header"}))
            .unwrap();
        use std::error::Error;
        for error in [
            exporter.flush().unwrap_err(),
            exporter
                .emit(serde_json::json!({"record":"end"}))
                .unwrap_err(),
            exporter.finish().unwrap_err(),
        ] {
            let source = error
                .source()
                .unwrap()
                .downcast_ref::<Arc<OTelSdkError>>()
                .unwrap();
            assert!(
                matches!(source.as_ref(), OTelSdkError::InternalFailure(message) if message == "delivery failed")
            );
        }
    }
    #[derive(Debug)]
    struct CollectContext(Arc<Mutex<Vec<Option<opentelemetry_sdk::logs::TraceContext>>>>);

    impl LogExporter for CollectContext {
        async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
            self.0.lock().unwrap().extend(
                batch
                    .iter()
                    .map(|(record, _)| record.trace_context().cloned()),
            );
            Ok(())
        }
    }

    #[test]
    fn snapshot_fragments_preserve_active_span_context_without_inventing_missing_context() {
        use opentelemetry::trace::{
            SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
        };
        let collected = Arc::new(Mutex::new(Vec::new()));
        let mut exporter = SnapshotExporter::with_exporter(
            workflow(),
            crate::identity::RunId::new(),
            CollectContext(collected.clone()),
        );
        let context = SpanContext::new(
            TraceId::from_bytes([1; 16]),
            SpanId::from_bytes([2; 8]),
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );
        {
            let _guard = opentelemetry::Context::new()
                .with_remote_span_context(context.clone())
                .attach();
            exporter
                .emit(serde_json::json!({"payload":"x".repeat(100_000)}))
                .unwrap();
        }
        exporter.emit(serde_json::json!({"record":"end"})).unwrap();
        exporter.finish().unwrap();
        let records = collected.lock().unwrap();
        assert!(records.len() > 2);
        for record in &records[..records.len() - 1] {
            let record = record.as_ref().unwrap();
            assert_eq!(record.trace_id, context.trace_id());
            assert_eq!(record.span_id, context.span_id());
            assert_eq!(record.trace_flags, Some(TraceFlags::SAMPLED));
        }
        assert!(
            records
                .last()
                .unwrap()
                .as_ref()
                .is_none_or(|context| context.trace_id == TraceId::INVALID
                    && context.span_id == SpanId::INVALID)
        );
    }
}

#[derive(Debug, Snafu)]
pub enum StreamDataExportError {
    #[snafu(transparent)]
    Setup { source: SetupError },
    #[snafu(transparent)]
    DataPacket {
        source: Arc<crate::stream_data::DataError>,
    },
    #[snafu(display("stream data capture requires an OTLP logs endpoint"))]
    DataMissingEndpoint,
    #[snafu(display("could not start stream data exporter: {source}"))]
    DataWorker { source: std::io::Error },
    #[snafu(display("stream data exporter is closed"))]
    DataClosed,
    #[snafu(display("stream data export failed: {source}"))]
    DataDelivery {
        source: Arc<opentelemetry_sdk::error::OTelSdkError>,
    },
    #[snafu(display("stream data shutdown exceeded its deadline"))]
    DataTimeout,
}

struct DataExportState {
    queue: std::collections::VecDeque<crate::stream_data::EncodedDataRecord>,
    bytes: usize,
    closing: Option<std::time::Instant>,
    done: bool,
    dropped: u64,
    fault: Option<Arc<StreamDataExportError>>,
}

struct DataExportShared {
    state: std::sync::Mutex<DataExportState>,
    changed: std::sync::Condvar,
    error: std::sync::Arc<std::sync::Mutex<Option<Arc<opentelemetry_sdk::error::OTelSdkError>>>>,
}

#[derive(Clone)]
pub struct StreamDataSender(Arc<DataExportShared>);

impl StreamDataSender {
    pub fn available(&self) -> bool {
        let state = self
            .0
            .state
            .lock()
            .expect("data export state was not poisoned");
        state.closing.is_none()
            && !state.done
            && state.queue.len() < crate::stream_data::MAX_QUEUE_RECORDS
            && state.bytes < crate::stream_data::MAX_QUEUE_BYTES
    }

    /// A full telemetry queue never becomes business backpressure.
    pub fn try_emit(
        &self,
        record: crate::stream_data::EncodedDataRecord,
    ) -> Result<bool, Arc<StreamDataExportError>> {
        self.diagnostic().map_or(Ok(()), Err)?;
        let mut state = self
            .0
            .state
            .lock()
            .expect("data export state was not poisoned");
        if state.closing.is_some() || state.done {
            return Err(Arc::new(DataClosedSnafu.build()));
        }
        let bytes = record.retained_bytes();
        if state.queue.len() == crate::stream_data::MAX_QUEUE_RECORDS
            || bytes > crate::stream_data::MAX_QUEUE_BYTES.saturating_sub(state.bytes)
        {
            state.dropped = state.dropped.saturating_add(1);
            return Ok(false);
        }
        state.bytes += bytes;
        state.queue.push_back(record);
        self.0.changed.notify_one();
        Ok(true)
    }

    pub fn dropped(&self) -> u64 {
        self.0
            .state
            .lock()
            .expect("data export state was not poisoned")
            .dropped
    }

    pub fn diagnostic(&self) -> Option<Arc<StreamDataExportError>> {
        let fault = self
            .0
            .state
            .lock()
            .expect("data export state was not poisoned")
            .fault
            .clone();
        fault.or_else(|| {
            self.0
                .error
                .lock()
                .expect("data delivery state was not poisoned")
                .clone()
                .map(|source| Arc::new(DataDeliverySnafu.into_error(source)))
        })
    }
}

/// The only blocking SDK processor lives on the dedicated export worker.
pub struct StreamDataExporter {
    sender: StreamDataSender,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl StreamDataExporter {
    pub fn from_env() -> Result<Self, StreamDataExportError> {
        ensure!(
            configured("OTEL_EXPORTER_OTLP_ENDPOINT")
                || configured("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"),
            DataMissingEndpointSnafu
        );
        let client = reqwest::blocking::Client::builder()
            .timeout(EXPORT_TIMEOUT)
            .build()
            .context(HttpClientSnafu)?;
        let exporter = LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(EXPORT_TIMEOUT)
            .with_http_client(client)
            .with_max_request_body_size(crate::stream_data::MAX_QUEUE_BYTES)
            .build()
            .context(LogsSnafu)?;
        Self::with_exporter(exporter)
    }

    fn with_exporter<E: opentelemetry_sdk::logs::LogExporter + 'static>(
        exporter: E,
    ) -> Result<Self, StreamDataExportError> {
        use opentelemetry::logs::LoggerProvider as _;
        let error = Arc::new(std::sync::Mutex::new(None));
        let logs = SdkLoggerProvider::builder()
            .with_resource(
                Resource::builder()
                    .with_service_name("mf-generated-workflow")
                    .build(),
            )
            .with_simple_exporter(SnapshotLogExporter {
                inner: exporter,
                error: error.clone(),
            })
            .build();
        let logger = logs.logger(crate::stream_data::SCOPE);
        let shared = Arc::new(DataExportShared {
            state: std::sync::Mutex::new(DataExportState {
                queue: std::collections::VecDeque::new(),
                bytes: 0,
                closing: None,
                done: false,
                dropped: 0,
                fault: None,
            }),
            changed: std::sync::Condvar::new(),
            error,
        });
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("stream-data-export".into())
            .spawn(move || {
                use opentelemetry::logs::Logger as _;
                loop {
                    let packet = {
                        let mut state = worker_shared
                            .state
                            .lock()
                            .expect("data export state was not poisoned");
                        loop {
                            if state
                                .closing
                                .is_some_and(|deadline| std::time::Instant::now() >= deadline)
                                || worker_shared
                                    .error
                                    .lock()
                                    .expect("data delivery state was not poisoned")
                                    .is_some()
                                || state.fault.is_some()
                            {
                                state.dropped =
                                    state.dropped.saturating_add(state.queue.len() as u64);
                                state.queue.clear();
                                state.bytes = 0;
                                break None;
                            }
                            if let Some(packet) = state.queue.pop_front() {
                                state.bytes -= packet.retained_bytes();
                                break Some(packet);
                            }
                            if state.closing.is_some() {
                                break None;
                            }
                            state = worker_shared
                                .changed
                                .wait(state)
                                .expect("data export state was not poisoned");
                        }
                    };
                    let Some(packet) = packet else {
                        break;
                    };
                    let mut record = logger.create_log_record();
                    if let Err(error) = packet.write_to(&mut record) {
                        worker_shared
                            .state
                            .lock()
                            .expect("data export state was not poisoned")
                            .fault
                            .get_or_insert_with(|| {
                                Arc::new(StreamDataExportError::from(Arc::new(error)))
                            });
                        continue;
                    }
                    // One bounded record per export also bounds request framing and worker memory.
                    logger.emit(record);
                }
                if let Err(source) = logs.shutdown_with_timeout(EXPORT_TIMEOUT) {
                    worker_shared
                        .error
                        .lock()
                        .expect("data delivery state was not poisoned")
                        .get_or_insert_with(|| Arc::new(source));
                }
                worker_shared
                    .state
                    .lock()
                    .expect("data export state was not poisoned")
                    .done = true;
                worker_shared.changed.notify_all();
            })
            .context(DataWorkerSnafu)?;
        Ok(Self {
            sender: StreamDataSender(shared),
            worker: Some(worker),
        })
    }

    pub fn sender(&self) -> StreamDataSender {
        self.sender.clone()
    }

    pub fn finish(&mut self) -> Result<(), Arc<StreamDataExportError>> {
        let deadline = std::time::Instant::now() + EXPORT_TIMEOUT;
        let mut state = self
            .sender
            .0
            .state
            .lock()
            .expect("data export state was not poisoned");
        state.closing.get_or_insert(deadline);
        self.sender.0.changed.notify_all();
        while !state.done {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                // The detached worker still has finite HTTP/shutdown deadlines.
                self.worker.take();
                drop(state);
                let error = self
                    .sender
                    .diagnostic()
                    .unwrap_or_else(|| Arc::new(DataTimeoutSnafu.build()));
                self.sender
                    .0
                    .state
                    .lock()
                    .expect("data export state was not poisoned")
                    .fault
                    .get_or_insert(error.clone());
                return Err(error);
            }
            state = self
                .sender
                .0
                .changed
                .wait_timeout(state, remaining)
                .expect("data export state was not poisoned")
                .0;
        }
        drop(state);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.sender.diagnostic().map_or(Ok(()), Err)
    }
}

impl Drop for StreamDataExporter {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let mut state = self
                .sender
                .0
                .state
                .lock()
                .expect("data export state was not poisoned");
            state.closing = Some(std::time::Instant::now());
            self.sender.0.changed.notify_all();
        }
    }
}

#[cfg(test)]
mod stream_data_tests {
    use super::*;
    use crate::stream_data::{DataPayload, DataRecord, SCHEMA_VERSION};
    use opentelemetry_sdk::{
        error::{OTelSdkError, OTelSdkResult},
        logs::{LogBatch, LogExporter},
    };
    use std::sync::{Mutex, mpsc};

    #[derive(Debug)]
    struct Collect {
        records: Arc<Mutex<Vec<DataRecord>>>,
        entered: Option<mpsc::Sender<()>>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }
    impl LogExporter for Collect {
        async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
            if let Some(release) = self.release.lock().unwrap().take() {
                self.entered.as_ref().unwrap().send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            for (record, scope) in batch.iter() {
                assert_eq!(scope.name(), crate::stream_data::SCOPE);
                let Some(opentelemetry::logs::AnyValue::Bytes(bytes)) = record.body() else {
                    panic!("missing bytes")
                };
                let decoded = DataRecord::decode(bytes).unwrap();
                if let Some(expected) = &decoded.trace_context {
                    let actual = record.trace_context().expect("native data context");
                    assert_eq!(actual.trace_id.to_string(), expected.trace_id);
                    assert_eq!(actual.span_id.to_string(), expected.span_id);
                    assert_eq!(actual.trace_flags.unwrap().to_u8(), expected.trace_flags);
                }
                self.records.lock().unwrap().push(decoded);
            }
            Ok(())
        }
    }
    fn packet(sequence: u64) -> crate::stream_data::EncodedDataRecord {
        DataRecord {
            version: SCHEMA_VERSION,
            workflow_id: crate::identity::WorkflowId::try_from(format!(
                "sha256:{}",
                "a".repeat(64)
            ))
            .unwrap(),
            run_id: crate::identity::RunId::new(),
            sequence,
            time_unix_nano: 1,
            trace_context: None,
            payload: DataPayload::Status {
                counts: Default::default(),
            },
        }
        .encode()
        .unwrap()
    }
    #[test]
    fn stream_diagnostic_releases_queue_state_before_reading_sdk_failure() {
        let sender = StreamDataSender(Arc::new(DataExportShared {
            state: Mutex::new(DataExportState {
                queue: Default::default(),
                bytes: 0,
                closing: None,
                done: false,
                dropped: 0,
                fault: None,
            }),
            changed: Default::default(),
            error: Arc::new(Mutex::new(None)),
        }));
        let delivery = sender.0.error.lock().unwrap();
        let reader = sender.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(reader.diagnostic()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        let capacity = sender.clone();
        let (capacity_tx, capacity_rx) = mpsc::channel();
        let capacity_worker = std::thread::spawn(move || {
            capacity_tx.send(capacity.available()).unwrap();
        });
        let queue_available = capacity_rx.recv_timeout(Duration::from_secs(1));
        drop(delivery);
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .is_none()
        );
        worker.join().unwrap();
        capacity_worker.join().unwrap();
        assert_eq!(
            queue_available,
            Ok(true),
            "SDK diagnostics must not stall capacity checks"
        );
    }

    #[test]
    fn stream_queue_pressure_does_not_wait_for_export() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let records = Arc::new(Mutex::new(Vec::new()));
        let mut exporter = StreamDataExporter::with_exporter(Collect {
            records: records.clone(),
            entered: Some(entered_tx),
            release: Mutex::new(Some(release_rx)),
        })
        .unwrap();
        let sender = exporter.sender();
        assert!(sender.try_emit(packet(0)).unwrap());
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for sequence in 1..=crate::stream_data::MAX_QUEUE_RECORDS {
            assert!(sender.try_emit(packet(sequence as u64)).unwrap());
        }
        assert!(!sender.try_emit(packet(1000)).unwrap());
        assert_eq!(sender.dropped(), 1);
        assert!(!sender.available());
        release_tx.send(()).unwrap();
        exporter.finish().unwrap();
        assert_eq!(
            records.lock().unwrap().len(),
            crate::stream_data::MAX_QUEUE_RECORDS + 1
        );
        assert!(matches!(
            sender.try_emit(packet(1001)).unwrap_err().as_ref(),
            StreamDataExportError::DataClosed
        ));
    }
    #[test]
    fn stream_worker_preserves_context_captured_on_the_producer_thread() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let mut exporter = StreamDataExporter::with_exporter(Collect {
            records: records.clone(),
            entered: None,
            release: Mutex::new(None),
        })
        .unwrap();
        let mut original = DataRecord::decode(&packet(0).bytes).unwrap();
        original.trace_context = Some(crate::wire::TraceContext {
            trace_id: "01".repeat(16),
            span_id: "02".repeat(8),
            trace_flags: 1,
        });
        exporter
            .sender()
            .try_emit(original.encode().unwrap())
            .unwrap();
        exporter.finish().unwrap();
        assert_eq!(*records.lock().unwrap(), vec![original]);
    }
    #[derive(Debug)]
    struct Fail;
    impl LogExporter for Fail {
        async fn export(&self, _: LogBatch<'_>) -> OTelSdkResult {
            Err(OTelSdkError::InternalFailure("data delivery failed".into()))
        }
    }
    #[test]
    fn stream_queue_bounds_bytes_before_record_capacity() {
        use crate::stream_data::{DataValue, Direction};
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut exporter = StreamDataExporter::with_exporter(Collect {
            records: Arc::new(Mutex::new(Vec::new())),
            entered: Some(entered_tx),
            release: Mutex::new(Some(release_rx)),
        })
        .unwrap();
        let sender = exporter.sender();
        sender.try_emit(packet(0)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut large = DataRecord::decode(&packet(1).bytes).unwrap();
        large.payload = DataPayload::Ports {
            node: "producer".into(),
            scope: Vec::new(),
            direction: Direction::Output,
            port_names: vec!["value".into()],
            ports_truncated: false,
            values: Some(DataValue::Object(std::collections::BTreeMap::from([(
                "value".into(),
                DataValue::String("x".repeat(200_000)),
            )]))),
            omission: None,
        };
        let large = large.encode().unwrap();
        let mut admitted = 0;
        while sender.try_emit(large.clone()).unwrap() {
            admitted += 1;
        }
        assert!(admitted > 0 && admitted < crate::stream_data::MAX_QUEUE_RECORDS);
        assert!(sender.0.state.lock().unwrap().bytes <= crate::stream_data::MAX_QUEUE_BYTES);
        assert_eq!(sender.dropped(), 1);
        release_tx.send(()).unwrap();
        exporter.finish().unwrap();
    }

    #[test]
    fn stream_shutdown_does_not_join_a_stalled_exporter_indefinitely() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut exporter = StreamDataExporter::with_exporter(Collect {
            records: Arc::new(Mutex::new(Vec::new())),
            entered: Some(entered_tx),
            release: Mutex::new(Some(release_rx)),
        })
        .unwrap();
        let sender = exporter.sender();
        sender.try_emit(packet(0)).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let error = exporter.finish().unwrap_err();
        assert!(matches!(error.as_ref(), StreamDataExportError::DataTimeout));
        assert!(matches!(
            sender.diagnostic().unwrap().as_ref(),
            StreamDataExportError::DataTimeout
        ));
        release_tx.send(()).unwrap();
    }
    #[test]
    fn stream_delivery_failure_keeps_its_typed_cause() {
        let mut exporter = StreamDataExporter::with_exporter(Fail).unwrap();
        let sender = exporter.sender();
        sender.try_emit(packet(0)).unwrap();
        let error = exporter.finish().unwrap_err();
        assert!(
            matches!(error.as_ref(), StreamDataExportError::DataDelivery { source }
            if matches!(source.as_ref(), OTelSdkError::InternalFailure(message) if message == "data delivery failed"))
        );
        assert!(matches!(
            sender.try_emit(packet(1)).unwrap_err().as_ref(),
            StreamDataExportError::DataDelivery { .. }
        ));
    }
}
