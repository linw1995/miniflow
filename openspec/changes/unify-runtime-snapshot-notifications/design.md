## Context

`SnapshotRecorder` currently owns a value dictionary, snapshot roots, history,
and encoding sink. `StreamDataObserver` is already a raw callback, but duplicates
the context field and ordinary task capture boundary. Their execution role is
the same; storage policy belongs to a caller that can choose either backend.

## Goals / Non-Goals

Use one runtime recorder and one task capture boundary. Keep original values,
successful publication semantics, exact stream identities, live emissions, and
out-of-lock callbacks. Preserve finite snapshot inspection and stream list UI.

Do not create a new crate or move the shared immutable value type. Existing
snapshot models and stores remain library utilities; the execution context only
holds a stateless callback. No replay, new UI controls, or transport redesign.

## Decisions

`SnapshotRecorder` invokes a callback with a `SnapshotEvent` containing scope,
node, current `NodeSnapshot`, and optional existing `DataIdentity`. It owns no
dictionary, historical values, encoding, or capture diagnostics. Optional run
binding is metadata validation shared by callers, not a stream-specific type.

The old stateful recorder becomes caller-owned `SnapshotRecording`. Its recorder
handle delegates events to that owner's existing dictionary/store/sink machinery.
Finite events keep their existing records and content deduplication. Stream
identity and emission outcome preserve distinct equal-valued notifications;
optional identity metadata is retained with historical entries and serialized
only when present. Existing finite records omit the new field.

Ordinary task input/start and publication/outcome notifications use `capture_node`
in both modes. Add an `Emitted` snapshot outcome for admitted outputs that do not
claim invocation completion. Producers/event callbacks use the same recorder,
release scheduler locks before notifications, and retain only temporary immutable
references for the current admission. No second recorder field or active-state
protocol remains in the runtime.

Stream telemetry adapts snapshot input and admitted output events to its existing
bounded protocol. Failed/skipped outcomes do not fabricate admitted outputs.
Runner owners finish finite recording or stream telemetry after execution.
The legacy whole-stream finite capture environment flag remains a runner-level
error because that flag selects the finite wire backend; programmatic snapshot
callbacks can observe either execution mode.

## Risks / Trade-offs

Snapshot notifications carry both input and output objects for ordinary tasks,
so enabled execution retains current input references until the outcome. It
retains no earlier invocations. Caller-owned recording can retain history by
explicit choice; using it for an indefinite stream requires caller policy.

Unbound callbacks can observe streams without telemetry identities. An explicitly
run-bound callback requires matching observation metadata. Callbacks must be
nonblocking and must not unwind; supplied adapters retain their typed fault policy.

## Validation

Adapt existing finite snapshot/history and stream integration tests. Extend an
existing live emission case to send equal values and verify distinct identities.
Verify raw snapshots without telemetry and finite numeric/sharing behavior.
Ablate notification deduplication or identity propagation to confirm assertions
detect semantic regressions. Keep reports and probes in ignored `target/`.

## Slow-test follow-up

The single-node runner and runtime derive compile cases currently rebuild local
fixtures in separate Cargo targets. Extend the existing nextest setup selection
and matching shared-Cargo group to include these cases. Keep each temporary
project and installed executable isolated, and preserve all assertions. The
existing wrapper redirects Cargo builds to the shared target; serialization
protects generated binaries until validation and installation finish.

Measure the same cases with the pinned local toolchain before and after, including
cache setup in the elapsed time. Do not add a second cache, harness, or tests that
merely mirror nextest configuration. Record timings under ignored `target/`.
