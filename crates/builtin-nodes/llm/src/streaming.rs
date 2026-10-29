use crate::{LlmEmission, LlmExecutionError, TextDelta, ToolCall, ToolCallEvent};
use futures::StreamExt;
use mf_runtime::{TypedEmitter, TypedNodeResult};
use rig_core::{
    completion::{AssistantContent, CompletionResponse},
    error::ProviderError,
    operation::Completion,
    streaming::{Item, StreamEvent, Streamed},
};
use snafu::{OptionExt, ResultExt, ensure};
use std::collections::BTreeMap;
use std::time::Instant;

pub fn complete(
    response: Result<Streamed<Completion>, ProviderError>,
    emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
    call_id: &str,
    deadline: Instant,
    context: &mf_runtime::ExecutionContext,
    tools: &[crate::ToolDefinition],
) -> Result<CompletionResponse, LlmExecutionError> {
    let allow_tools = !tools.is_empty();
    let mut response = response.with_context(|source| crate::ProviderSnafu {
        kind: source.kind().code(),
        status: source
            .provider_response_status()
            .map(|status| status.as_u16()),
    })?;
    let mut first_text = true;
    let mut arguments = BTreeMap::<usize, Vec<u8>>::new();
    let mut tool_count = 0usize;
    let mut argument_bytes = 0usize;
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
        if let Item::Event(event) = &item {
            match event {
                StreamEvent::Start {
                    part,
                    kind: rig_core::streaming::PartKind::ToolCall,
                } => {
                    tool_count += 1;
                    ensure!(
                        tool_count <= crate::tools::MAX_TOOL_CALLS,
                        crate::ToolLimitSnafu {
                            resource: "batch count"
                        }
                    );
                    arguments.insert(part.index(), Vec::new());
                }
                StreamEvent::Arguments { part, json } => {
                    let bytes =
                        arguments
                            .get_mut(&part.index())
                            .context(crate::InvalidResponseSnafu {
                                requirement: "complete unambiguous tool call fragments",
                            })?;
                    ensure!(
                        json.len()
                            <= crate::tools::MAX_TOOL_ARGUMENT_BYTES.saturating_sub(bytes.len()),
                        crate::ToolLimitSnafu {
                            resource: "argument bytes"
                        }
                    );
                    argument_bytes = argument_bytes.saturating_add(json.len());
                    ensure!(
                        argument_bytes <= crate::tools::MAX_TOOL_BATCH_ARGUMENT_BYTES,
                        crate::ToolLimitSnafu {
                            resource: "batch argument bytes"
                        }
                    );
                    bytes.extend_from_slice(json.as_bytes());
                }
                StreamEvent::End {
                    part,
                    content: rig_core::completion::AssistantContent::ToolCall(call),
                } => {
                    let bytes =
                        arguments
                            .remove(&part.index())
                            .context(crate::InvalidResponseSnafu {
                                requirement: "complete unambiguous tool call fragments",
                            })?;
                    let parsed: serde_json::Value = mf_runtime::decode_unique_json(&bytes)
                        .context(crate::ToolArgumentsJsonSnafu)?;
                    ensure!(
                        parsed == call.function.arguments,
                        crate::InvalidResponseSnafu {
                            requirement: "complete unambiguous tool call fragments"
                        }
                    );
                }
                _ => {}
            }
        }
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
                    call: {
                        let mut call = ToolCall::from_provider(call)?;
                        call.mark_arguments(tools);
                        call
                    },
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
    ensure!(
        arguments.is_empty(),
        crate::InvalidResponseSnafu {
            requirement: "complete tool call fragments"
        }
    );
    context
        .execute_future(response.finish(), Some(deadline))?
        .with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })
}
