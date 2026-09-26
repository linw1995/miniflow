use mf_telemetry::{
    observation::Observer,
    wire::{TraceContext, WireRecord},
};
use opentelemetry::{
    InstrumentationScope,
    logs::{AnyValue, Severity},
};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    logs::{LogProcessor, SdkLogRecord, SdkLoggerProvider},
    trace::{InMemorySpanExporter, Sampler, SdkTracerProvider},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default)]
pub struct Capture(pub Arc<Mutex<Vec<WireRecord>>>);

impl LogProcessor for Capture {
    fn emit(&self, record: &mut SdkLogRecord, scope: &InstrumentationScope) {
        self.0.lock().unwrap().push(WireRecord {
            scope: scope.name().into(),
            event_name: record.event_name().unwrap().into(),
            time_unix_nano: record
                .timestamp()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            trace_context: record.trace_context().map(|context| TraceContext {
                trace_id: context.trace_id.to_string(),
                span_id: context.span_id.to_string(),
                trace_flags: context.trace_flags.map(|flags| flags.to_u8()).unwrap_or(0),
            }),
            attributes: record
                .attributes_iter()
                .map(|(key, value)| (key.as_str().into(), value_json(value)))
                .collect(),
            body: value_json(record.body().unwrap()),
        });
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
    fn event_enabled(&self, _: Severity, _: &str, _: Option<&str>) -> bool {
        false
    }
}

fn value_json(value: &AnyValue) -> Value {
    match value {
        AnyValue::Int(value) => json!(value),
        AnyValue::String(value) => json!(value.as_str()),
        AnyValue::Boolean(value) => json!(value),
        AnyValue::ListAny(values) => values.iter().map(value_json).collect(),
        AnyValue::Map(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.as_str().into(), value_json(value)))
                .collect(),
        ),
        _ => panic!("unexpected lifecycle value"),
    }
}

pub struct Harness {
    pub capture: Capture,
    pub spans: InMemorySpanExporter,
    pub traces: SdkTracerProvider,
    pub logs: SdkLoggerProvider,
}

impl Harness {
    pub fn new(sample: bool) -> Self {
        let capture = Capture::default();
        let spans = InMemorySpanExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_sampler(if sample {
                Sampler::AlwaysOn
            } else {
                Sampler::AlwaysOff
            })
            .with_simple_exporter(spans.clone())
            .build();
        let logs = SdkLoggerProvider::builder()
            .with_log_processor(capture.clone())
            .build();
        Self {
            capture,
            spans,
            traces,
            logs,
        }
    }
    pub fn observer(&self) -> Observer {
        Observer::new(&self.traces, &self.logs)
    }
    pub fn records(&self) -> Vec<WireRecord> {
        self.capture.0.lock().unwrap().clone()
    }
}
