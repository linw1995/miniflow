## Why

Snapshot notifications are shared, but still carry streaming DataIdentity and
run binding. Existing native span context already correlates Data to invocations;
record timestamps and capture sequences already distinguish observations.
The snapshot boundary should remain independent of execution mode.

## What Changes

- Remove stream identities and run binding from snapshot events, storage, and handles.
- Preserve node scope, original port values, outcomes, and shared task capture.
- Simplify Data payloads to node, scope, direction, and values; reuse native span
  context, timestamps, and capture sequence without duplicating message provenance.
- Remove unused snapshot identity plumbing and the temporary event capture wrapper.
- Notify producer emissions only after successful admission, including raw callbacks.
- Adapt existing consumers and meaningful tests without adding parallel APIs.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: keep snapshots mode-independent and correlate inspected
  values through existing spans and recording metadata.

## Impact

Runtime snapshot APIs, stream Data payloads, telemetry adapters, generated runners,
TUI Data rendering, and existing tests/documentation change. These PR APIs and the
stream Data protocol are unreleased. Finite snapshot version 1 and stream lifecycle
schema 4 remain unchanged. Lifecycle message identities and transport budgets retain
their existing responsibilities.
