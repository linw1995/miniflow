## Why

[Issue #139](https://github.com/linw1995/miniflow/issues/139) requests business data in OTEL Tracing for streaming workflows. The current runtime rejects whole-run snapshots and the TUI always shows an unavailable message, so users cannot inspect the values processed or emitted by a stream invocation.

## What Changes

- Add opt-in, bounded streaming input/output capture alongside the existing lifecycle and finite snapshot protocols.
- Capture resolved inputs and each successfully admitted output separately, including multiple emissions from a running producer and outputs from timer or upstream-close callbacks.
- Export independently decodable OTLP LogRecords with invocation, message, and native trace/span correlation.
- Show a selectable, ordered data list in OTEL Tracing, with expanded values and explicit capture, omission, transport, and retention diagnostics.
- Enable stream data capture for TUI children while preserving the rejection of the unbounded whole-run snapshot recorder.
- Keep ordinary execution free of business-value export, and preserve finite snapshots, lifecycle schemas, execution results, and source backpressure behavior.

## Capabilities

### New Capabilities

None. Streaming capture extends the existing observation and terminal inspection boundaries.

### Modified Capabilities

- `workflow-observability`: Define optional streaming data records, capture boundaries, bounded export, correlation, and compatibility with finite snapshots.
- `workflow-terminal-ui`: Enable stream data capture and provide invocation-scoped and session-wide data lists with bounded retention and stable selection.

## Impact

- Runtime capture hooks in `mf-runtime` execution contexts, producer admission, and event callbacks; a separate optional recorder in `StreamOptions`.
- Stream data envelope and export support in `mf-telemetry`, without adding values to lifecycle records or span attributes.
- Generated runner setup in `mf-compiler`, and child configuration, ingestion, storage, and Data navigation in `mf-tui`.
- Focused runtime, protocol, receiver, rendering, and generated-runner acceptance coverage, plus `docs/observability.md` updates.
- No new external dependencies or change to the finite snapshot wire format is intended.
