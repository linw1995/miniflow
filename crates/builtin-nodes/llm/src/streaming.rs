use crate::{LlmEmission, LlmExecutionError, TextDelta};
use futures::StreamExt;
use mf_runtime::{TypedEmitter, TypedNodeResult};
use rig_core::{
    completion::CompletionResponse,
    error::ProviderError,
    operation::Completion,
    streaming::{Item, StreamEvent, Streamed},
};
use snafu::{ResultExt, ensure};
use std::time::Instant;

pub async fn complete(
    response: Result<Streamed<Completion>, ProviderError>,
    emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
    call_id: &str,
    limit: usize,
    deadline: Instant,
    context: &mf_runtime::ExecutionContext,
) -> Result<CompletionResponse, LlmExecutionError> {
    let mut response = response.with_context(|source| crate::ProviderSnafu {
        kind: source.kind().code(),
        status: source
            .provider_response_status()
            .map(|status| status.as_u16()),
    })?;
    let mut first_text = true;
    let mut bytes = 0usize;
    while let Some(item) = response.next().await {
        context.check_future_limits(Some(deadline))?;
        let item = item.with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })?;
        if let Item::Event(StreamEvent::Text { text, .. }) = item
            && !text.is_empty()
        {
            if first_text {
                context.record_event(
                    crate::FIRST_TEXT_EVENT,
                    serde_json::json!({"call_id": call_id}),
                );
                first_text = false;
            }
            ensure!(
                text.len() <= limit.saturating_sub(bytes),
                crate::OutputTooLargeSnafu { limit }
            );
            bytes += text.len();
            emitter
                .send_with_deadline(
                    TypedNodeResult::from(LlmEmission {
                        delta: TextDelta {
                            text,
                            model_call_id: call_id.into(),
                        },
                    }),
                    deadline,
                )
                .context(crate::PublicationSnafu)?;
        }
    }
    response
        .finish()
        .await
        .with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })
}
