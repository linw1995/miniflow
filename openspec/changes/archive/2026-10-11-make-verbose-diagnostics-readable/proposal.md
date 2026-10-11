# Readable Verbose Diagnostics

## Why

Verbose execution currently prints repeated routing attributes and JSON bodies. Users need to scan progress, outcomes,
and timing without decoding the OTLP record layout. Long-running nodes also need visible progress while no lifecycle
boundary is emitted.

## What Changes

- Render concise lifecycle descriptions with elapsed and node-duration milliseconds.
- Show the run ID once and preserve node, Loop, stream, port, and failure context in readable text.
- Escape control characters and keep each event on one line.
- Remind users every 60 seconds while a node is running, including telemetry-disabled execution.
- Keep the same validated lifecycle records and preserve OTLP export and telemetry-disabled local output.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: readable local diagnostics from the existing lifecycle records.

## Impact

Local stderr presentation, a private reminder worker, focused formatting and reminder regressions, existing runner
acceptance assertions, and usage documentation change. Runtime scheduling, wire schemas, SDK initialization, stdout,
and process outcomes remain intact.
