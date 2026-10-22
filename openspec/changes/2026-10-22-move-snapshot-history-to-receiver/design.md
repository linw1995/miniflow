## Context

Runtime execution outputs and branch snapshots are required execution state. Observation
history is separate. The runtime already publishes redacted node events at common
finite/stream boundaries; it does not need to own their encoding or retention.

## Decisions

- Put generic node-event and wire contracts in `mf-telemetry::capture`; specialize
  values as `ValueRef` in runtime aliases, avoiding a telemetry/runtime dependency cycle.
- Keep the publisher limited to a serialized sink, completion state, and typed diagnostic.
  Validate port objects before sending. Hold no prior payloads or value identities.
- Retain persistent scope/node roots in `mf-tui::snapshots::SnapshotStore`. Preserve
  repeated invocation boundaries and reuse unchanged nodes, without global value interning.
- Upgrade record and transport versions to 2. Reject old runners explicitly.
- Preserve existing bounded OTLP batching, sequence validation, context correlation, and
  fragmented records. Encode large unsigned integers as reserved eight-byte OTLP values.
  Encode deep container subtrees as tagged JSON bytes to stay within protobuf recursion
  limits; enforce the same total body depth limit before publication and after decoding.
- Compiler tests collect events locally instead of retaining a production runtime history API.

## Tradeoffs

Complete records can repeat large payloads on the wire. Receiver history still grows
with the session, which is intentional. Disk persistence, historical eviction, and
receiver-side global interning are not introduced by this change.

## Migration

Recompile runners. Replace `SnapshotRecording` callbacks with `SnapshotRecorder` and
an observation publisher or an application-owned event collector. Snapshot value IDs
and dictionary definitions no longer exist in the protocol.
