//! Source-timestamped observations shared by local collectors and OTel exporters.

use crate::{
    ContractError,
    identity::{RunId, WorkflowId},
    wire::TraceContext,
};
use opentelemetry::{
    Context,
    logs::{AnyValue, LogRecord},
    trace::TraceContextExt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const SCOPE: &str = "mf.observation";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub name: String,
    pub time_unix_nano: u64,
    pub body: Value,
    pub trace_context: Option<TraceContext>,
}

impl EventRecord {
    pub fn now(name: &'static str, body: Value) -> Option<Self> {
        let time_unix_nano = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos(),
        )
        .ok()?;
        let context = Context::current();
        let span = context.span();
        let correlation = span.span_context();
        let trace_context = correlation.is_valid().then(|| TraceContext {
            trace_id: correlation.trace_id().to_string(),
            span_id: correlation.span_id().to_string(),
            trace_flags: correlation.trace_flags().to_u8(),
        });
        Some(Self {
            name: name.into(),
            time_unix_nano,
            body,
            trace_context,
        })
    }

    pub fn write_to(
        &self,
        name: &'static str,
        workflow: &WorkflowId,
        run: RunId,
        record: &mut impl LogRecord,
    ) -> Result<(), ContractError> {
        crate::require(self.name == name, "observation event name changed")?;
        record.set_event_name(name);
        record.set_timestamp(UNIX_EPOCH + Duration::from_nanos(self.time_unix_nano));
        record.set_body(crate::wire::any_value(self.body.clone())?);
        record.add_attributes([
            (
                "mf.workflow.id",
                AnyValue::String(workflow.to_string().into()),
            ),
            ("mf.run.id", AnyValue::String(run.to_string().into())),
        ]);
        if let Some(context) = &self.trace_context {
            let (trace, span) = context.ids()?;
            record.set_trace_context(
                trace,
                span,
                Some(opentelemetry::TraceFlags::new(context.trace_flags)),
            );
        }
        Ok(())
    }
}

type CollectEvent = Arc<dyn Fn(&EventRecord) + Send + Sync>;
type EmitEvent = Arc<dyn Fn(&'static str, &EventRecord) + Send + Sync>;

/// Collect source events in the current observation context and its descendants.
pub fn collect(
    collect: impl Fn(&EventRecord) + Send + Sync + 'static,
) -> opentelemetry::ContextGuard {
    Context::current()
        .with_value::<CollectEvent>(Arc::new(collect))
        .attach()
}

/// Record one source timestamp for local collection and the active observation exporter.
pub fn record(name: &'static str, body: Value) -> Option<u64> {
    let event = EventRecord::now(name, body)?;
    let context = Context::current();
    if let Some(collect) = context.get::<CollectEvent>() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collect(&event)));
    }
    if let Some(emit) = context.get::<EmitEvent>() {
        emit(name, &event);
    }
    Some(event.time_unix_nano)
}

pub(super) fn with_emitter(
    context: Context,
    emit: impl Fn(&'static str, &EventRecord) + Send + Sync + 'static,
) -> Context {
    context.with_value::<EmitEvent>(Arc::new(emit))
}
