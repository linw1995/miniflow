## Why

Snapshot encoding and historical storage live in the runtime despite being observation
concerns. Bounding the producer dictionary adds cache policy without removing that
coupling. Replace the closed PR #189 approach with self-contained observations and
receiver-owned history.

## What Changes

- Keep runtime snapshot callbacks stateless and redacted.
- Publish complete node records through an observation-owned publisher.
- Move immutable history storage to the TUI receiver.
- Define the current snapshot contract without runtime dictionary/history APIs.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: receiver-owned history and self-contained snapshot records.

## Impact

The project has not released, so the implementation adopts the current contracts
directly. Applications collect runtime events or use observation-side storage.
Repeated values can consume more transport bandwidth; producer retention no longer
grows with distinct values.
