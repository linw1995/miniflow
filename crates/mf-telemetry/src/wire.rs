//! Logical OTel field mapping. JSON fixtures are not OTLP/HTTP request envelopes.

use crate::{
    ContractError, EVENT_SCHEMA_VERSION, INSTRUMENTATION_SCOPE, LOOP_EVENT_SCHEMA_VERSION,
    event::{Event, LifecycleEvent},
    require,
};
use opentelemetry::{
    SpanId, TraceFlags, TraceId,
    logs::{AnyValue, LogRecord},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use snafu::ResultExt;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub trace_flags: u8,
}

impl TraceContext {
    pub fn validate(&self) -> Result<(), ContractError> {
        self.ids().map(|_| ())
    }
    fn ids(&self) -> Result<(TraceId, SpanId), ContractError> {
        let trace = TraceId::from_hex(&self.trace_id).context(crate::TraceIdSnafu)?;
        let span = SpanId::from_hex(&self.span_id).context(crate::SpanIdSnafu)?;
        require(
            trace != TraceId::INVALID
                && span != SpanId::INVALID
                && trace.to_string() == self.trace_id
                && span.to_string() == self.span_id,
            "trace context must contain nonzero canonical lowercase IDs",
        )?;
        Ok((trace, span))
    }
}

/// A transport-independent view of the OTel fields owned by the lifecycle schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireRecord {
    pub scope: String,
    pub event_name: String,
    pub time_unix_nano: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<TraceContext>,
    pub attributes: Map<String, Value>,
    pub body: Value,
}

impl WireRecord {
    pub fn from_event(
        event: &LifecycleEvent,
        time_unix_nano: u64,
        trace_context: Option<TraceContext>,
    ) -> Result<Self, ContractError> {
        Self::from_event_with_version(event, EVENT_SCHEMA_VERSION, time_unix_nano, trace_context)
    }

    pub fn from_event_with_version(
        event: &LifecycleEvent,
        schema_version: i64,
        time_unix_nano: u64,
        trace_context: Option<TraceContext>,
    ) -> Result<Self, ContractError> {
        event.validate()?;
        require(
            matches!(
                schema_version,
                EVENT_SCHEMA_VERSION | LOOP_EVENT_SCHEMA_VERSION
            ),
            "unsupported event schema version",
        )?;
        validate_event_version(&event.event, schema_version)?;
        if let Some(context) = &trace_context {
            context.ids()?;
        }
        let mut encoded = serde_json::to_value(&event.event)?;
        let body = encoded["body"]
            .as_object_mut()
            .expect("event bodies are objects");
        let mut attributes = Map::from_iter([
            ("mf.schema.version".into(), json!(schema_version)),
            ("mf.workflow.id".into(), json!(event.workflow_id)),
            ("mf.run.id".into(), json!(event.run_id)),
            ("mf.event.sequence".into(), json!(event.sequence)),
        ]);
        if let Some(node) = body.remove("node") {
            attributes.insert("mf.node.id".into(), node["id"].clone());
            attributes.insert("mf.node.kind".into(), node["kind"].clone());
            if let Some(path) = node.get("path") {
                require(
                    schema_version == LOOP_EVENT_SCHEMA_VERSION,
                    "old event schema cannot contain a Loop path",
                )?;
                body.insert("loop_path".into(), path.clone());
            }
        }
        if matches!(
            event.event,
            Event::NodeFinished { .. } | Event::WorkflowFinished { .. }
        ) {
            if let Some(outcome) = body.remove("outcome") {
                attributes.insert("mf.outcome".into(), outcome);
            }
        } else if matches!(event.event, Event::NodeSkipped { .. }) {
            attributes.insert("mf.outcome".into(), json!("skipped"));
        }
        if let Some(failure) = body.get_mut("failure") {
            let phase = failure
                .as_object_mut()
                .expect("failure is an object")
                .remove("phase")
                .expect("failure has a phase");
            attributes.insert("mf.failure.phase".into(), phase);
        }
        Ok(Self {
            scope: INSTRUMENTATION_SCOPE.into(),
            event_name: event.event.name().into(),
            time_unix_nano,
            trace_context,
            attributes,
            body: Value::Object(body.clone()),
        })
    }

    pub fn decode(&self) -> Result<LifecycleEvent, ContractError> {
        require(
            self.scope == INSTRUMENTATION_SCOPE,
            "unexpected instrumentation scope",
        )?;
        let schema_version = self.schema_version()?;
        if let Some(context) = &self.trace_context {
            context.ids()?;
        }
        let mut body = self
            .body
            .as_object()
            .ok_or_else(|| crate::invalid("event body must be a structured map"))?
            .clone();
        require(
            !body.contains_key("node")
                && (!body.contains_key("outcome") || self.event_name == "mf.loop.pass.finished"),
            "routing fields belong in attributes",
        )?;
        if self.event_name.starts_with("mf.node.") {
            let path = body.remove("loop_path");
            require(
                schema_version == LOOP_EVENT_SCHEMA_VERSION || path.is_none(),
                "old event schema cannot contain a Loop path",
            )?;
            body.insert(
                "node".into(),
                match path {
                    Some(path) => json!({"id":self.attribute("mf.node.id")?, "kind":self.attribute("mf.node.kind")?, "path":path}),
                    None => json!({"id":self.attribute("mf.node.id")?, "kind":self.attribute("mf.node.kind")?}),
                },
            );
        } else {
            require(
                !self.attributes.contains_key("mf.node.id")
                    && !self.attributes.contains_key("mf.node.kind"),
                "workflow event contains node routing fields",
            )?;
        }
        match self.event_name.as_str() {
            "mf.node.skipped" => require(
                self.attribute("mf.outcome")?.as_str() == Some("skipped"),
                "skip outcome must be skipped",
            )?,
            "mf.node.finished" | "mf.workflow.finished" => {
                body.insert("outcome".into(), self.attribute("mf.outcome")?.clone());
            }
            _ => require(
                !self.attributes.contains_key("mf.outcome"),
                "nonterminal event contains an outcome",
            )?,
        }
        if let Some(failure) = body.get_mut("failure") {
            let failure = failure
                .as_object_mut()
                .ok_or_else(|| crate::invalid("failure must be an object"))?;
            require(
                !failure.contains_key("phase"),
                "failure phase belongs in attributes",
            )?;
            failure.insert("phase".into(), self.attribute("mf.failure.phase")?.clone());
            require(
                matches!(
                    self.event_name.as_str(),
                    "mf.node.finished" | "mf.workflow.finished"
                ),
                "nonterminal event contains failure context",
            )?;
        } else {
            require(
                !self.attributes.contains_key("mf.failure.phase"),
                "failure phase requires context",
            )?;
        }
        let result = LifecycleEvent {
            workflow_id: serde_json::from_value(self.attribute("mf.workflow.id")?.clone())?,
            run_id: serde_json::from_value(self.attribute("mf.run.id")?.clone())?,
            sequence: serde_json::from_value(self.attribute("mf.event.sequence")?.clone())?,
            event: serde_json::from_value(json!({"event_name":self.event_name,"body":body}))?,
        };
        result.validate()?;
        validate_event_version(&result.event, schema_version)?;
        Ok(result)
    }

    pub fn schema_version(&self) -> Result<i64, ContractError> {
        let version = self
            .attribute("mf.schema.version")?
            .as_i64()
            .ok_or_else(|| crate::invalid("invalid event schema version"))?;
        require(
            matches!(version, EVENT_SCHEMA_VERSION | LOOP_EVENT_SCHEMA_VERSION),
            "unsupported event schema version",
        )?;
        Ok(version)
    }

    fn attribute(&self, key: &str) -> Result<&Value, ContractError> {
        self.attributes
            .get(key)
            .ok_or_else(|| crate::invalid(format!("missing attribute {key}")))
    }

    /// Fills a fresh record created by a logger with the `mf.workflow` scope.
    /// It does not install a provider or emit, enqueue, or export the record.
    pub fn write_to(&self, record: &mut impl LogRecord) -> Result<(), ContractError> {
        let (normalized, name) = if matches!(
            self.attributes
                .get("mf.schema.version")
                .and_then(Value::as_i64),
            Some(crate::LEGACY_STREAM_EVENT_SCHEMA_VERSION | crate::STREAM_EVENT_SCHEMA_VERSION)
        ) {
            let event = crate::stream::StreamRecord::decode(self)?;
            (
                event.to_wire(self.time_unix_nano, self.trace_context.clone())?,
                event.payload.name(),
            )
        } else {
            let event = self.decode()?;
            (
                Self::from_event_with_version(
                    &event,
                    self.schema_version()?,
                    self.time_unix_nano,
                    self.trace_context.clone(),
                )?,
                event.event.name(),
            )
        };
        let timestamp = UNIX_EPOCH
            .checked_add(Duration::from_nanos(self.time_unix_nano))
            .ok_or_else(|| crate::invalid("timestamp exceeds platform range"))?;
        let body = any_value(normalized.body)?;
        let attributes = normalized
            .attributes
            .into_iter()
            .map(|(key, value)| Ok((key, any_value(value)?)))
            .collect::<Result<Vec<_>, ContractError>>()?;
        record.set_event_name(name);
        record.set_timestamp(timestamp);
        record.set_body(body);
        record.add_attributes(attributes);
        if let Some(context) = &self.trace_context {
            let (trace, span) = context.ids()?;
            record.set_trace_context(trace, span, Some(TraceFlags::new(context.trace_flags)));
        }
        Ok(())
    }
}

fn validate_event_version(event: &Event, schema_version: i64) -> Result<(), ContractError> {
    if schema_version == EVENT_SCHEMA_VERSION {
        require(
            !matches!(
                event,
                Event::LoopPassStarted { .. } | Event::LoopPassFinished { .. }
            ),
            "old event schema cannot contain Loop passes",
        )?;
        require(
            event.node().is_none_or(|(node, _)| node.path.is_empty()),
            "old event schema cannot contain a Loop path",
        )?;
        if let Event::NodeFinished { loop_summary, .. } = event {
            require(
                loop_summary.is_none(),
                "old event schema cannot contain a Loop summary",
            )?;
        }
        if let Event::WorkflowFinished {
            top_level_visited_count,
            ..
        } = event
        {
            require(
                top_level_visited_count.is_none(),
                "old event schema cannot contain a Loop visited prefix",
            )?;
        }
    }
    Ok(())
}

fn any_value(value: Value) -> Result<AnyValue, ContractError> {
    Ok(match value {
        Value::String(value) => value.into(),
        Value::Bool(value) => value.into(),
        Value::Number(value) => AnyValue::Int(
            value
                .as_i64()
                .ok_or_else(|| crate::invalid("OTel lifecycle numbers must be signed integers"))?,
        ),
        Value::Array(values) => AnyValue::ListAny(Box::new(
            values
                .into_iter()
                .map(any_value)
                .collect::<Result<_, _>>()?,
        )),
        Value::Object(values) => AnyValue::Map(Box::new(
            values
                .into_iter()
                .map(|(key, value)| Ok((key.into(), any_value(value)?)))
                .collect::<Result<_, ContractError>>()?,
        )),
        Value::Null => {
            return Err(crate::invalid(
                "optional lifecycle values must be absent, not null",
            ));
        }
    })
}
