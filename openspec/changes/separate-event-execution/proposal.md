# Proposal

## Why

Event-driven providers should construct their mutable executor directly without implementing an
unsupported synchronous task method. The merged preparation contract supplies the shared metadata;
execution kind now needs to be explicit before a graph is runnable.

## What Changes

- Add separate task and event execution variants to prepared nodes.
- Keep synchronous flows and generated bodies typed to task execution.
- Allow event state to be `Send` without requiring `Sync`.

## Impact

Plugin factories and direct task-helper callers migrate to the explicit execution variant. Existing
single-run behavior remains covered by the same compiler, generated-runner, and observation suites.
Streaming schemas, scheduling, collection policy, transport, and stream observation follow separately.
