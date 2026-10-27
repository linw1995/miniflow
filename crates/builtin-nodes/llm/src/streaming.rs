use crate::{LlmEmission, LlmExecutionError, TextDelta, ToolCall, ToolCallEvent};
use futures::StreamExt;
use mf_runtime::{TypedEmitter, TypedNodeResult};
use rig_core::{
    completion::{AssistantContent, CompletionResponse},
    error::ProviderError,
    operation::Completion,
    streaming::{Item, StreamEvent, Streamed},
};
use snafu::ResultExt;
use std::time::Instant;

pub fn complete(
    response: Result<Streamed<Completion>, ProviderError>,
    emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
    call_id: &str,
    deadline: Instant,
    context: &mf_runtime::ExecutionContext,
    allow_tools: bool,
) -> Result<CompletionResponse, LlmExecutionError> {
    let mut response = response.with_context(|source| crate::ProviderSnafu {
        kind: source.kind().code(),
        status: source
            .provider_response_status()
            .map(|status| status.as_u16()),
    })?;
    let mut first_text = true;
    // Keep synchronous publication outside the future driver's deadline check so
    // a delivery failure cannot be replaced with a capturable provider timeout.
    while let Some(item) = context.execute_future(response.next(), Some(deadline))? {
        context.check_future_limits(Some(deadline))?;
        let item = item.with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })?;
        let emission = match item {
            Item::Event(StreamEvent::Text { text, .. }) if !text.is_empty() => {
                if first_text {
                    mf_telemetry::events::record(
                        crate::FIRST_TEXT_EVENT,
                        serde_json::json!({"call_id": call_id}),
                    );
                    first_text = false;
                }
                LlmEmission {
                    delta: Some(TextDelta {
                        text,
                        model_call_id: call_id.into(),
                    }),
                    tool_call: None,
                }
            }
            Item::Event(StreamEvent::End {
                content: AssistantContent::ToolCall(call),
                ..
            }) if allow_tools => LlmEmission {
                delta: None,
                tool_call: Some(ToolCallEvent {
                    model_call_id: call_id.into(),
                    call: ToolCall::from_provider(call)?,
                }),
            },
            _ => continue,
        };
        let skipped = if emission.delta.is_some() {
            "tool_call"
        } else {
            "delta"
        };
        emitter
            .send(TypedNodeResult {
                outputs: emission,
                skipped: std::collections::BTreeSet::from([skipped.into()]),
                loop_summary: None,
            })
            .context(crate::PublicationSnafu)?;
    }
    context
        .execute_future(response.finish(), Some(deadline))?
        .with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })
}
