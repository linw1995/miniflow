# Stream tool-call arguments and fold tool results in chat

## Why

Chat should show the arguments a model is still generating without waiting for the tool-call batch to finish. Large tool results should not fill the conversation window by default.

## What changes

- Expose raw provider argument fragments through an optional `tool_call_delta` stream, without changing the completed `tool_call` stream.
- Deliver invocation-scoped argument previews through the existing conversation preview transport without changing settlement or model context.
- Derive live tool execution status from trace-correlated Flow call observations, independently of the model stream.
- Group authoritative tool results with their calls and collapse each result by default, with per-result keyboard navigation and expansion.

## Non-goals

- Model-provided execution status, changes to dispatch ordering, or early execution from previews.
- Streaming tool results, persisting execution observations or previews, or reading business data from telemetry snapshots.
- Interpreting tool results as Markdown, terminal commands, or image requests.
