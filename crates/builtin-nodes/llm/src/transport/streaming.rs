use super::*;
use futures::StreamExt;
use rig_core::http_client::BoxedStream;
use snafu::{Snafu, ensure};

#[derive(Debug, Snafu)]
pub enum StreamingTransportError {
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

pub fn normalize_stream(body: BoxedStream, usage: Arc<Mutex<Option<Usage>>>) -> BoxedStream {
    Box::pin(futures::stream::try_unfold(
        Frames {
            body,
            pending: Bytes::new(),
            frame: Vec::new(),
            usage,
        },
        |mut state| async move {
            let next = async move {
                loop {
                    // Ready-only SSE traffic must yield so the executor can observe cancellation.
                    let mut yielded = false;
                    futures::future::poll_fn(|context| {
                        if std::mem::replace(&mut yielded, true) {
                            std::task::Poll::Ready(())
                        } else {
                            context.waker().wake_by_ref();
                            std::task::Poll::Pending
                        }
                    })
                    .await;
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
    use mf_runtime::{ExecutionContext, FutureExecutionError, StreamCancellation};
    use std::time::{Duration, Instant};

    #[test]
    fn ready_metadata_frames_allow_executor_cancellation() {
        let cancellation = StreamCancellation::default();
        let mut context = ExecutionContext::default();
        context.set_cancellation(cancellation.clone());
        let mut count = 0;
        let body = Box::pin(
            futures::stream::repeat(Bytes::from_static(b": heartbeat\n\n"))
                .inspect(move |_| {
                    count += 1;
                    if count == 2 {
                        cancellation.cancel(
                            mf_runtime::StreamExecutionSnafu {
                                message: "stop metadata stream",
                            }
                            .build(),
                        );
                    }
                })
                .map(Ok),
        );
        let mut frames = normalize_stream(body, Arc::new(Mutex::new(None)));
        let result = context.execute_future(async { while frames.next().await.is_some() {} }, None);
        assert!(matches!(
            result,
            Err(FutureExecutionError::Cancelled { .. })
        ));
    }

    #[test]
    fn ready_metadata_frames_allow_executor_deadline_checks() {
        let body =
            Box::pin(futures::stream::repeat(Bytes::from_static(b": heartbeat\n\n")).map(Ok));
        let mut frames = normalize_stream(body, Arc::new(Mutex::new(None)));
        let result = ExecutionContext::default().execute_future(
            async { while frames.next().await.is_some() {} },
            Some(Instant::now() + Duration::from_millis(10)),
        );
        assert!(matches!(
            result,
            Err(FutureExecutionError::DeadlineExceeded)
        ));
    }
}
