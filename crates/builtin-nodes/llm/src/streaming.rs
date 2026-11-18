use crate::{
    LlmEmission, LlmExecutionError, TextDelta, ToolCall, ToolCallEvent,
    transport::TerminalTransport,
};
use futures::StreamExt;
use mf_runtime::{TypedEmitter, TypedNodeResult};
use rig_core::{
    completion::{AssistantContent, CompletionResponse},
    error::ProviderError,
    operation::Completion,
    streaming::{Item, StreamEvent, Streamed},
};
use snafu::ResultExt;
use std::{future::poll_fn, task::Poll, time::Instant};

pub fn complete(
    response: Result<Streamed<Completion>, ProviderError>,
    transport: &TerminalTransport,
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
    let mut first_output = true;
    let mut observe_output = || {
        if std::mem::take(&mut first_output) {
            mf_telemetry::events::record(
                crate::FIRST_OUTPUT_EVENT,
                serde_json::json!({"call_id": call_id}),
            );
        }
    };
    loop {
        let (arguments, item) = context.execute_future(
            poll_fn(|cx| {
                // Rig polls the body inline but may not yield an event until it assembles the call.
                // Hand captured wire fragments back to the synchronous publisher on the same poll.
                let item = response.poll_next_unpin(cx);
                let arguments = transport.take_tool_arguments();
                if item.is_ready() || !arguments.is_empty() {
                    Poll::Ready((arguments, item))
                } else {
                    Poll::Pending
                }
            }),
            Some(deadline),
        )?;
        // Keep synchronous publication outside the future driver. Delivery errors must
        // not be replaced with capturable timeouts, and publishing can pause body reads.
        for delta in arguments {
            context.check_future_limits(Some(deadline))?;
            observe_output();
            publish(
                emitter,
                LlmEmission {
                    delta: None,
                    tool_call: None,
                    tool_call_delta: Some(delta),
                },
            )?;
        }
        let item = match item {
            Poll::Pending => continue,
            Poll::Ready(None) => break,
            Poll::Ready(Some(item)) => item,
        };
        context.check_future_limits(Some(deadline))?;
        let item = item.with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })?;
        let emission = match item {
            Item::Event(StreamEvent::Text { text, .. }) if !text.is_empty() => {
                observe_output();
                LlmEmission {
                    delta: Some(TextDelta {
                        text,
                        model_call_id: call_id.into(),
                    }),
                    tool_call: None,
                    tool_call_delta: None,
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
                tool_call_delta: None,
            },
            _ => continue,
        };
        publish(emitter, emission)?;
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

fn publish(
    emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
    emission: LlmEmission,
) -> Result<(), LlmExecutionError> {
    let skipped = [
        ("delta", emission.delta.is_none()),
        ("tool_call", emission.tool_call.is_none()),
        ("tool_call_delta", emission.tool_call_delta.is_none()),
    ]
    .into_iter()
    .filter(|(_, absent)| *absent)
    .map(|(name, _)| name.into())
    .collect();
    emitter
        .send(TypedNodeResult {
            outputs: emission,
            skipped,
            loop_summary: None,
        })
        .context(crate::PublicationSnafu)
}
