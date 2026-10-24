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

type RecordEvent = dyn Fn(&EventRecord) + Send + Sync;

#[derive(Clone)]
pub struct EventRecorder(Arc<RecordEvent>);

impl std::fmt::Debug for EventRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventRecorder")
            .finish_non_exhaustive()
    }
}

impl EventRecorder {
    pub fn new(record: impl Fn(&EventRecord) + Send + Sync + 'static) -> Self {
        Self(Arc::new(record))
    }

    pub fn record(&self, event: &EventRecord) {
        // Optional observers cannot change a workflow's execution result.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.0)(event)));
    }
}
