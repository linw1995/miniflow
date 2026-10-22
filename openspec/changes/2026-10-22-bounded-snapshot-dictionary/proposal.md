## Why

Long-running executions accumulate every distinct observed value in the producer-side snapshot deduplication dictionary, so caller-owned recordings grow with distinct values and execution scopes. Callers need an opt-in bound on producer memory without changing the snapshot wire format, receiver reconstruction, or existing unbounded behavior.

## What Changes

- Add `SnapshotRetention` (`Unbounded` default, `Bounded { max_values }`) and `SnapshotRecording::with_sink_and_retention` to `mf-runtime`; `with_sink` keeps unbounded behavior.
- Bound the producer's live deduplication dictionary with LRU eviction; an evicted value is defined again under a new value ID when it reappears.
- Keep the wire format, protocol version, receiver `SnapshotStore`, and `current()`/`history()` contracts unchanged; `value_count()` still counts total assigned IDs.
- Read `MF_SNAPSHOT_DICTIONARY_LIMIT` in generated runners: a valid `usize` enables bounded retention for OTLP snapshot capture, absent or invalid values fall back to unbounded, and invalid values print a diagnostic.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: opt-in bounded producer dictionary retention for caller-owned snapshot recording, with generated runner wiring.

## Non-goals

No receiver-side eviction, gap recovery, or retention change. No wire format, protocol version, or store contract change. No runtime-owned history container or capture queue. No TUI or exporter policy change.
