## Decisions

SnapshotEvent contains node, scope, and NodeSnapshot only. SnapshotRecorder owns
one callback; SnapshotRecording remains caller-owned dictionary/history/sink state.
SnapshotRecord and SnapshotEntry contain no streaming identity. Snapshot value
interning remains shared; Emitted notifications remain distinct even for equal values.
Runtime task scopes use the existing execution scope path in either mode.

Stream Data Ports payloads carry the same node and scope plus direction, port summary,
values, or omission. Existing trace_context, time_unix_nano, and capture sequence
provide correlation, time, and ordering. Native context is captured synchronously
inside the existing node span; asynchronous adapters must capture context before
queueing. Missing context permits session inspection but never guessed association.
No new invocation ID, metadata wrapper, callback type, or timestamp unique key is added.

Remove DataIdentity and snapshot-only data_identity accessors. Preserve lifecycle
StreamIdentity and message IDs because they describe execution and causality.
Keep independently decodable bounded stream values and existing finite snapshot
encoding; backend retention policy remains a caller choice, not snapshot mode logic.

Producer admission must succeed before an Emitted notification is sent. Event output
capture uses the existing per-callback vector of admitted outputs without an identity
wrapper. All callbacks remain outside scheduler locks; partial admitted outputs survive
later failures. No terminal lifecycle result is fabricated from an emission.

## Validation and ablation

Reuse the clean pinned macOS affected baseline at fdb12de and the persistent fixture
cache. Adapt existing tests for native span correlation, repeated equal outputs,
nested scopes, and session records without context. Extend the existing producer
failure case to inspect raw snapshots without telemetry so identity filtering cannot
hide rejected publication. Remove the admission guard experimentally to verify this
assertion fails. Keep reports/probes under ignored target; do not add tests mirroring
removed identity fields or nextest configuration.

Follow origin/main AGENTS.md at be0d366: run affected coverage and prek -a locally;
leave full-suite validation to CI and do not run nix flake check locally. Review compliance,
commit task groups separately, archive after review, then update PR 142 and inspect
final checks and review feedback directly after completion.
