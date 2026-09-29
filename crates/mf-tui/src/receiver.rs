//! Bounded loopback OTLP/HTTP reception for one local workflow session.

use crate::state::{SessionState, StateError, StateSnapshot};
use http_body_util::{BodyExt, Full};
use hyper::{
    Request, Response, StatusCode,
    body::{Bytes, Incoming},
    http::{Method, header},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use mf_telemetry::{
    INSTRUMENTATION_SCOPE,
    description::WorkflowDescription,
    identity::{RunId, WorkflowId},
    wire::{TraceContext, WireRecord},
};
use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::{AnyValue, KeyValue, any_value},
    logs::v1::LogRecord,
};
use prost::Message;
use serde_json::{Map, Value};
use snafu::Snafu;
use std::{
    convert::Infallible,
    io,
    net::TcpListener,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio::sync::{Semaphore, oneshot};

pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECORDS_PER_REQUEST: usize = 1024;
pub const MAX_ACTIVE_CONNECTIONS: usize = 16;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
pub const CONNECTION_LIFETIME: Duration = Duration::from_secs(5);
const MAX_VALUE_DEPTH: usize = 8;
const MAX_COLLECTION_ITEMS: usize = 1024;

#[derive(Debug, Snafu)]
pub enum ReceiverError {
    #[snafu(display("could not bind loopback OTLP receiver: {source}"))]
    Bind { source: io::Error },
    #[snafu(display("could not initialize OTLP receiver runtime: {source}"))]
    Runtime { source: io::Error },
    #[snafu(display("could not initialize workflow state: {source}"))]
    State { source: StateError },
}

struct Context {
    state: Arc<Mutex<SessionState>>,
    workflow_id: WorkflowId,
    run_id: String,
}

pub struct LoopbackReceiver {
    endpoint: String,
    context: Arc<Context>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl LoopbackReceiver {
    pub fn bind(description: WorkflowDescription, run_id: RunId) -> Result<Self, ReceiverError> {
        let workflow_id = description.workflow_id.clone();
        let state = SessionState::new(description, run_id)
            .map_err(|source| ReceiverError::State { source })?;
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|source| ReceiverError::Bind { source })?;
        let endpoint = format!(
            "http://{}",
            listener
                .local_addr()
                .map_err(|source| ReceiverError::Bind { source })?
        );
        listener
            .set_nonblocking(true)
            .map_err(|source| ReceiverError::Bind { source })?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|source| ReceiverError::Runtime { source })?;
        let listener = {
            let _entered = runtime.enter();
            tokio::net::TcpListener::from_std(listener)
                .map_err(|source| ReceiverError::Runtime { source })?
        };
        let context = Arc::new(Context {
            state: Arc::new(Mutex::new(state)),
            workflow_id,
            run_id: run_id.to_string(),
        });
        let (stop, stopped) = oneshot::channel();
        let shared = Arc::clone(&context);
        let worker = thread::spawn(move || runtime.block_on(serve(listener, shared, stopped)));
        Ok(Self {
            endpoint,
            context,
            stop: Some(stop),
            worker: Some(worker),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn snapshot(&self) -> StateSnapshot {
        self.context
            .state
            .lock()
            .expect("receiver state was not poisoned")
            .snapshot()
    }

    pub fn finish(&mut self) -> StateSnapshot {
        self.stop();
        let mut state = self
            .context
            .state
            .lock()
            .expect("receiver state was not poisoned");
        state.close();
        state.snapshot()
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for LoopbackReceiver {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn serve(
    listener: tokio::net::TcpListener,
    context: Arc<Context>,
    mut stopped: oneshot::Receiver<()>,
) {
    let permits = Arc::new(Semaphore::new(MAX_ACTIVE_CONNECTIONS));
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => {
                        note_error(&context, &format!("OTLP accept failed: {error}"));
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    context.state.lock().expect("receiver state was not poisoned")
                        .record_observation_error("OTLP connection limit exceeded");
                    continue;
                };
                let shared = Arc::clone(&context);
                tokio::spawn(async move {
                    let _permit = permit;
                    let service = service_fn(move |request| handle(request, Arc::clone(&shared)));
                    let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
                    let _ = tokio::time::timeout(CONNECTION_LIFETIME, connection).await;
                });
            }
        }
    }
}

async fn handle(
    request: Request<Incoming>,
    context: Arc<Context>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = request.uri().path().to_owned();
    if request.method() != Method::POST {
        return Ok(response(StatusCode::METHOD_NOT_ALLOWED, "POST required"));
    }
    if path != "/v1/logs" && path != "/v1/traces" {
        return Ok(response(StatusCode::NOT_FOUND, "unknown OTLP path"));
    }
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim() == "application/x-protobuf")
    {
        note_request_error(
            &context,
            &path,
            "OTLP request used an unsupported content type",
        );
        return Ok(response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "protobuf required",
        ));
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_REQUEST_BYTES)
    {
        note_request_error(&context, &path, "OTLP request body exceeded the size limit");
        return Ok(response(StatusCode::PAYLOAD_TOO_LARGE, "request too large"));
    }
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    let mut body = request.into_body();
    let mut bytes = Vec::new();
    loop {
        let frame = match tokio::time::timeout_at(deadline, body.frame()).await {
            Ok(Some(Ok(frame))) => frame,
            Ok(None) => break,
            Ok(Some(Err(_))) => {
                note_request_error(&context, &path, "invalid OTLP HTTP body");
                return Ok(response(StatusCode::BAD_REQUEST, "invalid HTTP body"));
            }
            Err(_) => {
                note_request_error(&context, &path, "OTLP request timed out");
                return Ok(response(StatusCode::REQUEST_TIMEOUT, "request timed out"));
            }
        };
        if let Ok(data) = frame.into_data() {
            if data.len() > MAX_REQUEST_BYTES.saturating_sub(bytes.len()) {
                note_request_error(&context, &path, "OTLP request body exceeded the size limit");
                return Ok(response(StatusCode::PAYLOAD_TOO_LARGE, "request too large"));
            }
            bytes.extend_from_slice(&data);
        }
    }
    let result = if path == "/v1/logs" {
        receive_logs(&bytes, &context)
    } else {
        receive_traces(&bytes, &context)
    };
    Ok(match result {
        Ok(()) => response(StatusCode::OK, ""),
        Err(error) => response(StatusCode::BAD_REQUEST, &error),
    })
}

fn response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            if status == StatusCode::OK {
                "application/x-protobuf"
            } else {
                "text/plain"
            },
        )
        .body(Full::new(Bytes::copy_from_slice(body.as_bytes())))
        .expect("static HTTP response is valid")
}

fn receive_logs(bytes: &[u8], context: &Context) -> Result<(), String> {
    let request = ExportLogsServiceRequest::decode(bytes).map_err(|error| {
        note_error(context, "invalid OTLP logs protobuf");
        error.to_string()
    })?;
    let total = request
        .resource_logs
        .iter()
        .flat_map(|resource| &resource.scope_logs)
        .map(|scope| scope.log_records.len())
        .fold(0usize, usize::saturating_add);
    if total > MAX_RECORDS_PER_REQUEST {
        let related = request
            .resource_logs
            .iter()
            .flat_map(|resource| &resource.scope_logs)
            .filter(|scope| {
                scope.scope.as_ref().map(|scope| scope.name.as_str()) == Some(INSTRUMENTATION_SCOPE)
            })
            .flat_map(|scope| &scope.log_records)
            .filter(|record| {
                string_attribute(&record.attributes, "mf.run.id") == Some(context.run_id.as_str())
                    && string_attribute(&record.attributes, "mf.workflow.id")
                        == Some(context.workflow_id.as_str())
            })
            .count();
        if related != 0 {
            context
                .state
                .lock()
                .expect("receiver state was not poisoned")
                .record_local_lifecycle_drop(
                    related as u64,
                    "OTLP log record count exceeded the limit",
                );
        }
        return Err("too many log records".into());
    }
    let mut rejected = false;
    for resource in request.resource_logs {
        for scope in resource.scope_logs {
            if scope.scope.as_ref().map(|scope| scope.name.as_str()) != Some(INSTRUMENTATION_SCOPE)
            {
                continue;
            }
            for record in scope.log_records {
                if let Err(error) = receive_log(record, context) {
                    rejected = true;
                    context
                        .state
                        .lock()
                        .expect("receiver state was not poisoned")
                        .record_diagnostic(&error);
                }
            }
        }
    }
    if rejected {
        Err("one or more lifecycle records were rejected".into())
    } else {
        Ok(())
    }
}

fn receive_log(record: LogRecord, context: &Context) -> Result<(), String> {
    let run = string_attribute(&record.attributes, "mf.run.id");
    let workflow = string_attribute(&record.attributes, "mf.workflow.id");
    if run != Some(context.run_id.as_str()) || workflow != Some(context.workflow_id.as_str()) {
        if run.is_some() && workflow.is_some() {
            context
                .state
                .lock()
                .expect("receiver state was not poisoned")
                .record_diagnostic("ignored lifecycle record from another session");
            return Ok(());
        }
        note_error(context, "lifecycle record omitted session identity");
        return Err("missing lifecycle session identity".into());
    }
    let attributes = lifecycle_attributes(&record.attributes).inspect_err(|_| {
        note_error(context, "lifecycle record has invalid attributes");
    })?;
    let body = record.body.as_ref().ok_or_else(|| {
        context
            .state
            .lock()
            .expect("receiver state was not poisoned")
            .record_local_lifecycle_drop(1, "missing lifecycle body");
        "missing lifecycle body".to_owned()
    })?;
    let body = value(body, 0).inspect_err(|_| {
        context
            .state
            .lock()
            .expect("receiver state was not poisoned")
            .record_local_lifecycle_drop(1, "invalid lifecycle body");
    })?;
    let trace_context = if record.trace_id.is_empty() && record.span_id.is_empty() {
        None
    } else {
        if record.trace_id.len() != 16 || record.span_id.len() != 8 {
            context
                .state
                .lock()
                .expect("receiver state was not poisoned")
                .record_local_lifecycle_drop(1, "invalid lifecycle trace context");
            return Err("invalid lifecycle trace context".into());
        }
        Some(TraceContext {
            trace_id: hex(&record.trace_id),
            span_id: hex(&record.span_id),
            trace_flags: (record.flags & 0xff) as u8,
        })
    };
    let wire = WireRecord {
        scope: INSTRUMENTATION_SCOPE.into(),
        event_name: record.event_name,
        time_unix_nano: record.time_unix_nano,
        trace_context,
        attributes,
        body,
    };
    let event = wire.decode().map_err(|error| {
        context
            .state
            .lock()
            .expect("receiver state was not poisoned")
            .record_local_lifecycle_drop(1, "invalid lifecycle schema");
        error.to_string()
    })?;
    let schema_version = wire.schema_version().map_err(|error| error.to_string())?;
    {
        let mut state = context
            .state
            .lock()
            .expect("receiver state was not poisoned");
        if schema_version != state.expected_event_schema_version() {
            state.record_local_lifecycle_drop(1, "event and description versions disagree");
            return Err("event and description versions disagree".into());
        }
    }
    let admission = context
        .state
        .lock()
        .expect("receiver state was not poisoned")
        .apply(event);
    match admission {
        Ok(_) => Ok(()),
        Err(error) => {
            context
                .state
                .lock()
                .expect("receiver state was not poisoned")
                .record_local_lifecycle_drop(1, "lifecycle admission rejected a record");
            Err(error.to_string())
        }
    }
}

fn receive_traces(bytes: &[u8], context: &Context) -> Result<(), String> {
    let request = ExportTraceServiceRequest::decode(bytes).map_err(|error| {
        context
            .state
            .lock()
            .expect("receiver state was not poisoned")
            .record_local_trace_drop(1, "invalid OTLP traces protobuf");
        error.to_string()
    })?;
    let total = request
        .resource_spans
        .iter()
        .flat_map(|resource| &resource.scope_spans)
        .map(|scope| scope.spans.len())
        .fold(0usize, usize::saturating_add);
    if total > MAX_RECORDS_PER_REQUEST {
        let related = request
            .resource_spans
            .iter()
            .flat_map(|resource| &resource.scope_spans)
            .flat_map(|scope| &scope.spans)
            .filter(|span| {
                string_attribute(&span.attributes, "mf.run.id") == Some(context.run_id.as_str())
                    && string_attribute(&span.attributes, "mf.workflow.id")
                        == Some(context.workflow_id.as_str())
            })
            .count();
        if related != 0 {
            context
                .state
                .lock()
                .expect("receiver state was not poisoned")
                .record_local_trace_drop(related as u64, "OTLP span count exceeded the limit");
        }
        return Err("too many spans".into());
    }
    let mut rejected = false;
    for resource in request.resource_spans {
        for scope in resource.scope_spans {
            for span in scope.spans {
                let run = string_attribute(&span.attributes, "mf.run.id");
                let workflow = string_attribute(&span.attributes, "mf.workflow.id");
                if run != Some(context.run_id.as_str())
                    || workflow != Some(context.workflow_id.as_str())
                {
                    context
                        .state
                        .lock()
                        .expect("receiver state was not poisoned")
                        .record_diagnostic("ignored trace span from another session");
                    continue;
                }
                if span.trace_id.len() != 16 || span.span_id.len() != 8 {
                    context
                        .state
                        .lock()
                        .expect("receiver state was not poisoned")
                        .record_local_trace_drop(1, "invalid trace or span identity");
                    rejected = true;
                } else {
                    context
                        .state
                        .lock()
                        .expect("receiver state was not poisoned")
                        .record_trace_span();
                }
            }
        }
    }
    if rejected {
        Err("one or more spans were rejected".into())
    } else {
        Ok(())
    }
}

fn note_error(context: &Context, message: &str) {
    context
        .state
        .lock()
        .expect("receiver state was not poisoned")
        .record_observation_error(message);
}

fn note_request_error(context: &Context, path: &str, message: &str) {
    let mut state = context
        .state
        .lock()
        .expect("receiver state was not poisoned");
    if path == "/v1/traces" {
        state.record_local_trace_drop(1, message);
    } else {
        state.record_observation_error(message);
    }
}

fn string_attribute<'a>(attributes: &'a [KeyValue], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.key == name)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            any_value::Value::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

fn lifecycle_attributes(attributes: &[KeyValue]) -> Result<Map<String, Value>, String> {
    let mut result = Map::new();
    for attribute in attributes {
        if !matches!(
            attribute.key.as_str(),
            "mf.schema.version"
                | "mf.workflow.id"
                | "mf.run.id"
                | "mf.event.sequence"
                | "mf.node.id"
                | "mf.node.kind"
                | "mf.outcome"
                | "mf.failure.phase"
        ) {
            continue;
        }
        let item = attribute
            .value
            .as_ref()
            .ok_or("missing OTLP attribute value")?;
        if result
            .insert(attribute.key.clone(), value(item, 0)?)
            .is_some()
        {
            return Err("duplicate lifecycle attribute key".into());
        }
    }
    Ok(result)
}

fn value(input: &AnyValue, depth: usize) -> Result<Value, String> {
    if depth > MAX_VALUE_DEPTH {
        return Err("OTLP value exceeded nesting limit".into());
    }
    match input.value.as_ref().ok_or("missing OTLP value")? {
        any_value::Value::StringValue(value) => Ok(Value::String(value.clone())),
        any_value::Value::BoolValue(value) => Ok(Value::Bool(*value)),
        any_value::Value::IntValue(value) => Ok(Value::Number((*value).into())),
        any_value::Value::ArrayValue(values) => {
            if values.values.len() > MAX_COLLECTION_ITEMS {
                return Err("OTLP array exceeded item limit".into());
            }
            values
                .values
                .iter()
                .map(|item| value(item, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        any_value::Value::KvlistValue(values) => {
            if values.values.len() > MAX_COLLECTION_ITEMS {
                return Err("OTLP map exceeded item limit".into());
            }
            let mut map = Map::new();
            for item in &values.values {
                if item.key.is_empty() {
                    return Err("empty OTLP map key".into());
                }
                let entry = item.value.as_ref().ok_or("missing OTLP map value")?;
                if map
                    .insert(item.key.clone(), value(entry, depth + 1)?)
                    .is_some()
                {
                    return Err("duplicate OTLP map key".into());
                }
            }
            Ok(Value::Object(map))
        }
        _ => Err("unsupported OTLP value type for lifecycle metadata".into()),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    output
}
