use crate::{LlmEmission, LlmExecutionError, TextDelta};
use futures::StreamExt;
use mf_runtime::{TypedEmitter, TypedNodeResult};
use rig_core::{
    completion::CompletionResponse,
    error::ProviderError,
    operation::Completion,
    streaming::{Item, StreamEvent, Streamed},
};
use snafu::ResultExt;
use std::time::Instant;

pub async fn complete(
    response: Result<Streamed<Completion>, ProviderError>,
    emitter: &mut TypedEmitter<'_, '_, LlmEmission>,
    call_id: &str,
    deadline: Instant,
    context: &mf_runtime::ExecutionContext,
    timer: &mut crate::timing::CallTimer,
) -> Result<CompletionResponse, LlmExecutionError> {
    let mut response = response.with_context(|source| crate::ProviderSnafu {
        kind: source.kind().code(),
        status: source
            .provider_response_status()
            .map(|status| status.as_u16()),
    })?;
    while let Some(item) = response.next().await {
        context.check_future_limits(Some(deadline))?;
        let item = item.with_context(|source| crate::ProviderSnafu {
            kind: source.kind().code(),
            status: source
                .provider_response_status()
                .map(|status| status.as_u16()),
        })?;
        if let Item::Event(StreamEvent::Text { text, .. }) = item {
            if !text.is_empty() {
                timer.observe_text();
            }
            if !text.is_empty() {
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
