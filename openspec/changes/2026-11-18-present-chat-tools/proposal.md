# Present live tools in chat

## Why

Chat currently previews assistant text but withholds tool calls and results until final settlement. Long-running tools therefore appear silent, and large saved tool results overwhelm the conversation.

## What changes

- Publish optional typed tool-call fragments and execution lifecycle streams from the shared Agent.
- Deliver invocation-scoped tool previews through the existing acknowledged conversation transport without changing settlement or model context.
- Show receiving, queued, running, completed, failed, and interrupted tool states in the chat transcript.
- Collapse live and saved tool results by default, with per-result keyboard navigation and expansion.

## Non-goals

- Provider retries, altered tool selection, or speculative execution.
- Persisting previews or deriving business arguments from telemetry snapshots.
- Interpreting tool results as Markdown, terminal commands, or image requests.
