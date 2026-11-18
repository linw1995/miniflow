# Stream tool-call arguments and fold tool results in chat

## Why

Chat should show the arguments a model is still generating without waiting for the tool-call batch to finish. Large tool results should not fill the conversation window by default.

## What changes

- Expose raw provider argument fragments through an optional `tool_call_delta` stream, without changing the completed `tool_call` stream.
- Deliver invocation-scoped argument previews through the existing conversation preview transport without changing settlement or model context.
- Collapse authoritative tool results by default, with per-result keyboard navigation and expansion.

## Non-goals

- Tool execution status, changes to dispatch ordering, or early execution from previews.
- Persisting previews or reading business arguments from telemetry snapshots.
- Interpreting tool results as Markdown, terminal commands, or image requests.
