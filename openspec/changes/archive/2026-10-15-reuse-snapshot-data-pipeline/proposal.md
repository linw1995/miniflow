## Why

Snapshot notifications already describe the same business data in both execution modes,
but separate streaming codecs, exporters, stores, and UI adapters duplicate the finite
snapshot pipeline and introduce policies unrelated to runtime execution.

## What Changes

- Use SnapshotRecording, SnapshotExporter, and SnapshotCapture in both modes.
- Use MF_CAPTURE_SNAPSHOTS for every runner and expose the same session/invocation Data list.
- Remove the unreleased stream Data protocol, value model, queue, retention store, and redundant tests.
- Preserve native span correlation, timestamps, sequence ordering, equal emissions, decoded values,
  and explicit incomplete-history diagnostics using the existing snapshot protocol.
- Replace stream-only omission, eviction, and independent gap recovery guarantees with the existing
  snapshot contract: caller-owned history and dictionary encoding, bounded export backpressure,
  and ordered reconstruction. History can grow with collected data; runtime owns no history policy.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: share the complete snapshot capture and transport pipeline.
- `workflow-terminal-ui`: share snapshot storage and Data list inspection across execution modes.

## Impact

Generated runners, telemetry, runtime value adapters, TUI reception/rendering, tests, and docs.
Finite snapshot version 1 and stream lifecycle schema remain unchanged. The unreleased
MF_CAPTURE_STREAM_DATA switch and mf.stream.data protocol are removed.
