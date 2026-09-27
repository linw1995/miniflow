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
