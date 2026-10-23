use super::*;
use futures::StreamExt;
use rig_core::http_client::BoxedStream;
use snafu::{Snafu, ensure};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Snafu)]
pub enum StreamingTransportError {
    #[snafu(transparent)]
    Http {
        #[snafu(source(from(Error, Box::new)))]
        source: Box<Error>,
    },
    #[snafu(transparent)]
    Normalization { source: LlmExecutionError },
    #[snafu(display("LLM streaming response exceeds its {limit}-byte {kind} limit"))]
    Limit { kind: &'static str, limit: usize },
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
    total: usize,
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

pub fn bounded(body: BoxedStream, usage: Arc<Mutex<Option<Usage>>>) -> BoxedStream {
    Box::pin(futures::stream::try_unfold(
        Frames {
            body,
            pending: Bytes::new(),
            frame: Vec::new(),
            total: 0,
            usage,
        },
        |mut state| async move {
            let next = async move {
                loop {
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
                    ensure!(
                        length <= MAX_FRAME_BYTES - state.frame.len(),
                        LimitSnafu {
                            kind: "SSE event",
                            limit: MAX_FRAME_BYTES
                        }
                    );
                    ensure!(
                        length <= MAX_STREAM_BYTES - state.total,
                        LimitSnafu {
                            kind: "stream",
                            limit: MAX_STREAM_BYTES
                        }
                    );
                    state.total += length;
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
