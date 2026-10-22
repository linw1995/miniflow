## Why

Snapshot encoding and historical storage live in the runtime despite being observation
concerns. Bounding the producer dictionary adds cache policy without removing that
coupling. Replace the closed PR #189 approach with self-contained observations and
receiver-owned history.

## What Changes

- Keep runtime snapshot callbacks stateless and redacted.
- Publish complete node records through an observation-owned publisher.
- Move immutable history storage to the TUI receiver.
- Remove runtime dictionary/history APIs and introduce snapshot protocol version 2.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: receiver-owned history and self-contained snapshot records.

## Impact

Generated runners must be rebuilt for the new protocol. Runtime history API callers
must collect events or use observation-side storage. Repeated values can consume more
transport bandwidth; producer retention no longer grows with distinct values.
