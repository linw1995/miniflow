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
    tool_arguments: Option<Arc<ToolArgumentDeltas>>,
}

fn tool_argument_deltas(reply: &Value, model_call_id: &str) -> impl Iterator<Item = ToolCallDelta> {
    let primary = reply
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| {
            choices.iter().find(|choice| {
                choice.is_object()
                    && choice
                        .get("index")
                        .is_none_or(|index| index.is_null() || index.as_u64() == Some(0))
            })
        });
    primary
        .filter(|_| reply.get("object").and_then(Value::as_str) != Some("chat.completion"))
        .and_then(|choice| choice.pointer("/delta/tool_calls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(move |tool| {
            let function = tool.get("function");
            let arguments = function
                .and_then(|function| function.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let hint = |value: Option<&Value>| {
                value
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_owned)
            };
            let id = hint(tool.get("id"));
            let name = hint(function.and_then(|function| function.get("name")));
            if arguments.is_empty() && id.is_none() && name.is_none() {
                return None;
            }
            Some(ToolCallDelta {
                model_call_id: model_call_id.into(),
                index: tool.get("index").map_or(Some(0), Value::as_u64)?,
                arguments: arguments.to_owned(),
                id,
                name,
            })
        })
}

fn normalize_event(
    bytes: Vec<u8>,
    usage: &Mutex<Option<Usage>>,
    tool_arguments: Option<&ToolArgumentDeltas>,
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
    let reply: Value = mf_runtime::decode_unique_json(data.as_bytes()).context(JsonSnafu)?;
    // Rig's DTO requires complete counters. Keep sparse values out of that DTO.
    let normalized = normalize(Bytes::from(data), usage)?;
    if let Some(tool_arguments) = tool_arguments {
        tool_arguments
            .pending
            .lock()
            .unwrap()
            .extend(tool_argument_deltas(&reply, &tool_arguments.model_call_id));
    }
    let mut event = Vec::with_capacity(normalized.len() + 8);
    event.extend_from_slice(b"data: ");
    event.extend_from_slice(&normalized);
    event.extend_from_slice(b"\n\n");
    Ok(Bytes::from(event))
}

pub(super) fn normalize_stream(
    body: BoxedStream,
    usage: Arc<Mutex<Option<Usage>>>,
    tool_arguments: Option<Arc<ToolArgumentDeltas>>,
) -> BoxedStream {
    Box::pin(futures::stream::try_unfold(
        Frames {
            body,
            pending: Bytes::new(),
            frame: Vec::new(),
            usage,
            tool_arguments,
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
                        let frame = normalize_event(
                            std::mem::take(&mut state.frame),
                            &state.usage,
                            state.tool_arguments.as_deref(),
                        )?;
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
    fn tool_argument_deltas_are_raw_string_fragments_from_the_primary_choice_only() {
        use serde_json::json;
        let reply = json!({"choices":[
            {"index":1,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"other choice"}}]}},
            {"index":0,"delta":{"tool_calls":[
                {"index":2,"id":"id","function":{"name":"lookup","arguments":"{ \"query\":\t"}},
                {"index":2,"function":{"arguments":"\"é\" }"}},
                {"function":{"arguments":" "}},
                {"index":2,"id":" ","function":{"name":" ","arguments":"  "}},
                {"index":2,"function":{"arguments":""}},
                {"index":2,"function":{"arguments":{"query":"whole value"}}},
                {"index":2,"function":{"name":" "}},
                {"index":-1,"function":{"arguments":"invalid index"}},
                {"index":3,"function":null},
                {"index":3}
           ]}}
        ]});
        let fragments: Vec<_> = tool_argument_deltas(&reply, "model").collect();
        assert_eq!(fragments.len(), 4);
        assert!(fragments.iter().all(|delta| delta.model_call_id == "model"));
        assert_eq!(
            fragments
                .iter()
                .map(|delta| (delta.index, delta.arguments.as_str()))
                .collect::<Vec<_>>(),
            [(2, "{ \"query\":\t"), (2, "\"é\" }"), (0, " "), (2, "  ")]
        );
        assert_eq!(fragments[0].id.as_deref(), Some("id"));
        assert_eq!(fragments[0].name.as_deref(), Some("lookup"));
        assert!(
            fragments[1..]
                .iter()
                .all(|delta| delta.id.is_none() && delta.name.is_none())
        );
        for reply in [
            json!({"usage":{"total_tokens":3}}),
            json!({"choices":[null,{"index":1,"delta":{"tool_calls":[]}}]}),
            json!({"choices":[{"index":null,"delta":{}}]}),
            json!({"object":"chat.completion","choices":[{"delta":{"tool_calls":[{"function":{"arguments":"not an SSE delta"}}]}}]}),
        ] {
            assert!(tool_argument_deltas(&reply, "model").next().is_none());
        }
    }

    #[test]
    fn identity_hints_survive_frames_without_an_argument_fragment() {
        use serde_json::json;
        let reply = json!({"choices":[{"index":0,"delta":{"tool_calls":[
            {"index":1,"id":"separate-id"},
            {"index":1,"function":{"name":"lookup","arguments":""}},
            {"index":1,"function":{"arguments":"{ \"query\": "}},
            {"index":2,"id":" ","function":{"name":" ","arguments":""}}
        ]}}]});
        let deltas: Vec<_> = tool_argument_deltas(&reply, "model").collect();
        let [id, name, fragment] = deltas.as_slice() else {
            panic!("expected both hints and one argument fragment: {deltas:?}");
        };
        assert!(deltas.iter().all(|delta| delta.index == 1));
        assert!(id.arguments.is_empty() && name.arguments.is_empty());
        assert_eq!(id.id.as_deref(), Some("separate-id"));
        assert_eq!(name.name.as_deref(), Some("lookup"));
        assert_eq!(fragment.arguments, "{ \"query\": ");
        assert!(fragment.id.is_none() && fragment.name.is_none());
    }

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
        let mut frames = normalize_stream(body, Arc::new(Mutex::new(None)), None);
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
        let mut frames = normalize_stream(body, Arc::new(Mutex::new(None)), None);
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
