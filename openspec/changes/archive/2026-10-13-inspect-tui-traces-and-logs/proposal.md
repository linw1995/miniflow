## Why

The TUI receives OTLP traces but exposes only their count, while its value history hides the relationship between execution spans, lifecycle logs, and node data. Users need one observation view that preserves input/output inspection and correlates the two OpenTelemetry signals.

## What Changes

- Replace the separate Data History page with a trace tree and selectable Details, Data, and Logs panes.
- Retain bounded OTLP spans and LogRecords, including resource and instrumentation scope metadata.
- Show logs associated with a selected span and provide a session-wide log browser for records without a retained span.
- Preserve finite-workflow snapshot capture and display decoded node inputs/outputs using native trace/span correlation, including repeated Loop and Iteration work.
- Keep streaming snapshot restrictions and lifecycle integrity independent of raw trace/log retention.

## Capabilities

### Modified Capabilities

- `workflow-terminal-ui`: inspect spans, correlated logs, and decoded node data in one terminal observation view.
- `workflow-observability`: preserve native trace/span context on opt-in snapshot records.

## Impact

Changes affect `mf-tui` reception, retention, rendering, CLI terminal tests, snapshot export correlation, and observability documentation. Existing OTLP endpoints and snapshot body contracts remain compatible. No new dependencies or workflow execution semantics are required.
