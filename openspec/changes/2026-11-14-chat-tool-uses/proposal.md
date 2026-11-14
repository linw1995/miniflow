# Proposal

## Why

The current draft demonstrates live and saved tool records, but it treats telemetry as their only source, polls during
presentation/checkpointing, and rewrites one JSON column. Tool requests/results should instead follow the chat Flow's
structured messages, while runtime observations remain independently useful and visible. Their identities, authority,
and persistence must be designed together rather than concatenating two tool lists.

## What Changes

- Add optional current-invocation `messages` and typed message events to the shared chat contract. Existing workflows
  remain valid; internal full-context Agent messages must be adapted before being exposed as new messages.
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
- `chat-session-storage`: Source-aware relational tool records, incremental updates, migration, and recovery.
- `workflow-terminal-ui`: Event-driven, correlated message/observation presentation without duplicate or invented calls.

## Non-Goals

- Desktop, new graphical dependencies, settings forms, license-policy changes, or packaging.
- Guessing source associations from names/time, inferring success from tool-result text, or reconstructing lost messages.
- Automatically adding diagnostic records to model context or replacing authoritative final text with raw model text.

## Status

This is the revised plan requested during review. The existing metrics-only/polling/JSON implementation is the
baseline to replace, not evidence that this design is complete. New implementation and validation are pending.
