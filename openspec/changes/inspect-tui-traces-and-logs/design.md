## Context

See proposal.md for motivation. The receiver currently reduces lifecycle LogRecords, reconstructs `mf.snapshot` values, and counts completed spans without retaining their payloads. Snapshot records use value definitions and node references, with fragmentation and ordered assembly. The runtime captures nodes while their observation context is attached; the logging SDK normally supplies that context automatically.

## Goals / Non-Goals

Goals are exact invocation correlation, preserved finite data browsing, independent signal loss reporting, and bounded raw telemetry retention. This change does not add streaming value capture, remote attachment, persistent replay, or business payloads to span attributes.

## Decisions

### Retain spans and logs independently of lifecycle reduction

Keep resource and scope metadata with immutable span/log observations. Use trace/span identity for span deduplication and full-record fingerprints for log retransmissions. Bound each raw signal store to 4,096 records and 16 MiB of encoded record/resource/scope data; retain admitted records and count later overflow. Raw log retention failure must not prevent lifecycle admission or snapshot assembly. This preserves existing state semantics when browsing detail reaches capacity.

### Correlate data through native LogRecord context

Explicitly preserve the active trace/span context on snapshot export. Carry each snapshot sequence's
correlation through ordered/fragmented assembly, and associate decoded Node records with the
corresponding span. Value definitions continue using the existing shared store. Do not infer a
relationship from node name, arrival order, or timing: Loop passes and parallel Iteration items can
share names and overlap. Records without valid correlation remain available through session
logs/history diagnostics, and cannot supply another invocation's Data pane.

Keeping the existing log transport preserves large structured values and compatibility. Embedding complete input/output values in span attributes would duplicate data, encounter exporter attribute limits, and require a new payload contract.

### Present related signals together

The observation page keeps a span tree and a right-hand pane switched with Tab between Details,
Data, and Logs. Details shows span metadata, attributes, events, and links. Data shows the selected
invocation's decoded inputs/outputs, outcome, skipped ports, and capture diagnostics. Logs lists
records matching both trace and span IDs and allows selection for complete record inspection. A
dedicated session log mode exposes run-level records and logs whose spans are absent or unsampled.
Keyboard hints make each mode discoverable; graph navigation remains available through `v` or Esc.

Keep selections attached to stable identities when newly exported parents or logs reorder lists. Cache immutable snapshots and bounded previews so frame rendering does not repeatedly clone payloads. Sanitize terminal labels and bound span/log previews to 64 KiB; preserve the existing 64 KiB limit per input/output object.

### Preserve launch and failure behavior

Finite TUI children retain snapshot capture. Streaming children continue receiving capture disabled and show an explicit unsupported Data message while Details and Logs remain usable. Trace/log display drops and missing spans do not imply lifecycle loss or alter child results. Typed snapshot transport/store failures remain intact until presentation.

## Risks / Trade-offs

- Spans are exported after completion: live records are inspectable through session logs while the graph continues showing running work.
- Logs and spans may arrive in either order: correlation is retained independently, and pane caches refresh when the associated records/data change.
- A trace is sampled out or raw retention fills: session logs remain inspectable and losses are reported separately; no synthetic spans are created.
- Legacy snapshot records lack correlation: explain unavailable span data rather than assigning values by node name.
- Finite snapshots preserve existing whole-run retention: streaming data capture remains disabled to avoid introducing lifetime-dependent growth.

## Implementation and Commit Order

1. Add bounded span retention and deterministic trace-tree snapshots with focused receiver/store tests.
2. Add bounded LogRecord retention and exact snapshot correlation, with export/assembly/reordering tests.
3. Replace the history page with the combined observation UI, preserve finite capture, update terminal tests/docs, and complete final validation and OpenSpec synchronization.

Commit the proposal, design, delta specs, and unchecked task list first. Commit each numbered group separately after its validation, updating task completion in that group's commit.
