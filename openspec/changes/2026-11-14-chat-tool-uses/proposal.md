# Proposal

## Why

The current draft demonstrates live and saved tool records, but it treats telemetry as their only source, polls during
presentation/checkpointing, and rewrites one JSON column. Tool requests/results should instead follow the chat Flow's
structured messages, while runtime observations remain independently useful and visible. Their identities, authority,
and persistence must be designed together rather than concatenating two tool lists.

## What Changes

- Use the Agent's required complete `messages` input/output as the only chat contract, with optional immutable typed
  message events. Remove historical prompt/system/history binding and text-only result compatibility. Custom Flows
  may compact, reorder or replace history without a persistence counter, prefix slicing or second transcript.
- Persist every accepted Flow message as canonical ordered chat data, including message metadata, assistant tool calls,
  arguments, and tool results. Restore exactly the latest successful complete context after restart, retaining older
  versions and event evidence for inspection without resurrecting discarded history. ToolUse summaries are not recovery data.
- Derive semantic tool calls/results from messages. Preserve execution observations as a separate source of actual
  state, resolved arguments, and duration, including observation-only tools.
- Correlate only through explicit scoped identities. Merge proven matches into one semantic item with execution
  attempts; keep unlinked observations labeled separately and retain source-specific conflicts.
- Replace render/checkpoint polling with producer-driven events, bounded reduction, and independent display/storage
  subscribers. Hidden or paused drawing must not control capture, persistence, or cancellation.
- Replace the live model-call JSON column with associated semantic-call, execution, and small source-checkpoint rows.
  Write only changed rows/columns and preserve old draft records as observation-only evidence during migration.
- Keep terminal presentation literal, bounded, privacy-aware, and recoverable without rerunning models or tools.

## Capabilities

### Modified Capabilities

- `chat-executable-contract`: Optional structured message results/events and explicit per-invocation ordering.
- `chat-session-storage`: Durable structured messages and complete history reconstruction, linked tool/execution rows,
  incremental updates, migration, and recovery.
- `workflow-terminal-ui`: Event-driven, correlated message/observation presentation without duplicate or invented calls.

## Non-Goals

- Desktop, new graphical dependencies, settings forms, license-policy changes, or packaging.
- Guessing source associations from names/time, inferring success from tool-result text, or reconstructing lost messages.
- Automatically adding diagnostic records to model context or replacing authoritative final text with raw model text.

## Status

The review-requested implementation and local validation are complete, including Flow-owned full-context
replacement. See `verification.md` for evidence and the separately tracked CI gate; the PR remains Draft.
