## Why

The runtime still has separate finite snapshot and streaming data observer paths.
Moving stream encoding into telemetry left duplicate execution-context fields and
ordinary-task capture calls. Both paths observe current node values and can use
the existing snapshot boundary.

## What Changes

- Make `SnapshotRecorder` a shared callback for current snapshot events.
- Keep dictionary encoding, deduplication, diagnostics, and historical storage in
  caller-owned `SnapshotRecording`, alongside the existing snapshot data model.
- Route ordinary stream tasks through `capture_node`; notify each admitted
  producer/event emission through the same recorder.
- Remove `StreamDataObserver` and `StreamOptions.data_capture`.
- Carry optional stream identities without discarding repeated equal emissions.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: use one snapshot notification interface for finite and
  streaming execution, with retention and transport owned by callers.

## Impact

Runtime snapshot/context/stream APIs, generated runner wiring, existing snapshot
consumers, documentation, and integration tests change. The newly introduced PR
APIs change before release. Finite snapshot version 1 remains compatible when
optional stream metadata is absent; stream data wire and TUI behavior are unchanged.
