//! Caller-owned OTLP/HTTP providers for a standalone workflow process.

use crate::observation::Observer;
use opentelemetry_otlp::{LogExporter, Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    logs::{BatchConfigBuilder as LogBatchConfigBuilder, BatchLogProcessor, SdkLoggerProvider},
    trace::{BatchConfigBuilder as SpanBatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};
use snafu::{ResultExt, Snafu};
use std::{env, time::Duration};

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
        Observer::new(&self.traces, &self.logs)
    }

    /// Ends both pipelines within separate finite deadlines after a handled run.
    pub fn shutdown(self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if let Err(error) = self.logs.shutdown_with_timeout(EXPORT_TIMEOUT) {
            diagnostics.push(format!("OTLP logs: {error}"));
        }
        if let Err(error) = self.traces.shutdown_with_timeout(EXPORT_TIMEOUT) {
            diagnostics.push(format!("OTLP traces: {error}"));
        }
        diagnostics
    }
}

fn configured(key: &str) -> bool {
    env::var(key).is_ok_and(|value| !value.trim().is_empty())
}

const SNAPSHOT_BATCH_SIZE: usize = 64;

#[derive(Debug)]
struct SnapshotLogExporter<E> {
    inner: E,
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
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
            self.error
                .lock()
                .expect("snapshot exporter was not poisoned")
                .get_or_insert_with(|| error.to_string());
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
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    closed: bool,
}

impl SnapshotExporter {
    pub fn from_env(
        workflow: crate::identity::WorkflowId,
        run: crate::identity::RunId,
    ) -> Result<Self, String> {
        if !configured("OTEL_EXPORTER_OTLP_ENDPOINT")
            && !configured("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        {
            return Err("snapshot capture requires an OTLP logs endpoint".into());
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(EXPORT_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let inner = LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(EXPORT_TIMEOUT)
            .with_http_client(client)
            .with_max_request_body_size(MAX_REQUEST_BYTES)
            .build()
            .map_err(|error| error.to_string())?;
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

    pub fn emit(&mut self, body: serde_json::Value) -> Result<(), String> {
        use opentelemetry::logs::Logger as _;
        self.check_error()?;
        if self.closed {
            return Err("snapshot exporter is closed".into());
        }
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
            .ok_or("snapshot sequence overflow")?;
        Ok(())
    }

    fn check_error(&self) -> Result<(), String> {
        self.error
            .lock()
            .expect("snapshot exporter was not poisoned")
            .clone()
            .map_or(Ok(()), Err)
    }

    pub fn flush(&mut self) -> Result<(), String> {
        self.logs.force_flush().map_err(|error| error.to_string())?;
        self.buffered = 0;
        self.check_error()
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if self.closed {
            return self.check_error();
        }
        let result = self.flush();
        self.closed = true;
        let shutdown = self
            .logs
            .shutdown_with_timeout(EXPORT_TIMEOUT)
            .map_err(|error| error.to_string());
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
        assert!(exporter.flush().is_err());
        assert!(exporter.emit(serde_json::json!({"record":"end"})).is_err());
        assert!(exporter.finish().is_err());
    }
}
