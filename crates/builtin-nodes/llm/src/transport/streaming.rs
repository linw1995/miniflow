use super::*;
use futures::StreamExt;
use rig_core::http_client::BoxedStream;
use snafu::{Snafu, ensure};

#[derive(Debug, Snafu)]
pub enum StreamingTransportError {
    #[snafu(transparent)]
    Future {
        source: mf_runtime::FutureExecutionError,
    },
    #[snafu(transparent)]
    Http {
        #[snafu(source(from(Error, Box::new)))]
        source: Box<Error>,
    },
    #[snafu(transparent)]
    Normalization { source: LlmExecutionError },
    #[snafu(display("LLM streaming response contains an incomplete SSE event"))]
    TruncatedEvent,
    #[snafu(display("LLM streaming response contains invalid UTF-8"))]
    Utf8 { source: std::str::Utf8Error },
    #[snafu(display("LLM streaming response contains invalid JSON"))]
    Json { source: serde_json::Error },
}

struct Frames {
    body: BoxedStream,
    pending: Bytes,
    frame: Vec<u8>,
    usage: Arc<Mutex<Option<Usage>>>,
    cancellation: mf_runtime::StreamCancellation,
    deadline: std::time::Instant,
}

fn normalize_event(
    bytes: Vec<u8>,
    usage: &Mutex<Option<Usage>>,
) -> std::result::Result<Bytes, StreamingTransportError> {
    let text = std::str::from_utf8(&bytes).context(Utf8Snafu)?;
    let data = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() || data == "[DONE]" {
        return Ok(Bytes::from(bytes));
    }
    let _: Value = mf_runtime::decode_unique_json(data.as_bytes()).context(JsonSnafu)?;
    // Rig's DTO requires complete counters. Keep sparse values out of that DTO.
    let normalized = normalize(Bytes::from(data), usage)?;
    let mut event = Vec::with_capacity(normalized.len() + 8);
    event.extend_from_slice(b"data: ");
    event.extend_from_slice(&normalized);
    event.extend_from_slice(b"\n\n");
    Ok(Bytes::from(event))
}

pub fn normalize_stream(
    body: BoxedStream,
    usage: Arc<Mutex<Option<Usage>>>,
    cancellation: mf_runtime::StreamCancellation,
    deadline: std::time::Instant,
) -> BoxedStream {
    Box::pin(futures::stream::try_unfold(
        Frames {
            body,
            pending: Bytes::new(),
            frame: Vec::new(),
            usage,
            cancellation,
            deadline,
        },
        |mut state| async move {
            let next = async move {
                loop {
                    mf_runtime::check_future_limits(&state.cancellation, Some(state.deadline))?;
                    if state.pending.is_empty() {
                        let Some(chunk) = state.body.next().await else {
                            ensure!(state.frame.is_empty(), TruncatedEventSnafu);
                            return Ok::<_, StreamingTransportError>(None);
                        };
                        state.pending = chunk?;
                    }
                    let length = state
                        .pending
                        .iter()
                        .position(|byte| *byte == b'\n')
                        .map_or(state.pending.len(), |index| index + 1);
                    state
                        .frame
                        .extend_from_slice(&state.pending.split_to(length));
                    if state.frame.ends_with(b"\n\n") || state.frame.ends_with(b"\r\n\r\n") {
                        let frame =
                            normalize_event(std::mem::take(&mut state.frame), &state.usage)?;
                        return Ok(Some((frame, state)));
                    }
                }
            };
            next.await.map_err(Error::instance)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn metadata_only_frames_check_cancellation_between_deliveries() {
        let cancellation = mf_runtime::StreamCancellation::default();
        let body = Box::pin(futures::stream::iter([
            Ok(Bytes::from_static(b": heartbeat\n\n")),
            Ok(Bytes::from_static(b": heartbeat\n\n")),
        ]));
        let mut frames = normalize_stream(
            body,
            Arc::new(Mutex::new(None)),
            cancellation.clone(),
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        );
        let result = mf_runtime::ExecutionContext::default()
            .execute_future(
                async {
                    assert!(frames.next().await.unwrap().is_ok());
                    cancellation.cancel_with_message("stop metadata stream");
                    frames.next().await.unwrap().unwrap_err()
                },
                None,
            )
            .unwrap();
        assert!(result.source().unwrap().is::<StreamingTransportError>());
        assert!(matches!(
            result
                .source()
                .unwrap()
                .downcast_ref::<StreamingTransportError>(),
            Some(StreamingTransportError::Future {
                source: mf_runtime::FutureExecutionError::Cancelled { .. }
            })
        ));
    }

    #[test]
    fn metadata_only_frames_check_the_total_deadline_before_reading() {
        let body = Box::pin(futures::stream::iter([Ok(Bytes::from_static(
            b": heartbeat\n\n",
        ))]));
        let mut frames = normalize_stream(
            body,
            Arc::new(Mutex::new(None)),
            Default::default(),
            std::time::Instant::now() - std::time::Duration::from_millis(1),
        );
        let result = mf_runtime::ExecutionContext::default()
            .execute_future(async { frames.next().await.unwrap().unwrap_err() }, None)
            .unwrap();
        assert!(matches!(
            result
                .source()
                .unwrap()
                .downcast_ref::<StreamingTransportError>(),
            Some(StreamingTransportError::Future {
                source: mf_runtime::FutureExecutionError::DeadlineExceeded
            })
        ));
    }
}
