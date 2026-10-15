## Design

Keep runtime SnapshotRecorder notifications unchanged. Attach SnapshotRecording.recorder()
in either runner mode and finish the same recording after execution. Use the existing snapshot
exporter, including its backpressure and typed delivery errors; do not create a replacement queue.

SnapshotCapture reconstructs one shared SnapshotStore. Associate each admitted history entry
with native trace/span context, packet sequence, and timestamp at assembly time. Keep this
presentation metadata alongside the existing store; do not introduce another value/history model.
Session and invocation lists read these same entries in either mode. Existing preview rendering
shows decoded inputs, outputs, outcomes, skipped ports, and errors. Equal Emitted records remain
distinct. Missing context stays visible in session Data without guessed correlation.

Retain existing snapshot ordering, dictionary references, deduplication, and incomplete-tail
semantics. A missing definition can prevent later reconstruction; report this instead of inventing
independent recovery. History retention remains an upper-layer concern in both modes. Remove
stream-only budgets, omissions, eviction witnesses, and independently decodable payloads rather
than rebuilding them under snapshot names. No new capture IDs or execution identities are needed.

## Validation and ablation

Use origin/main AGENTS.md at be0d366. Run affected pinned-toolchain tests and prek -a;
CI owns complete-suite validation. Reuse existing snapshot transport, store, runtime admission,
and native correlation tests. Adapt stream runner integration to the shared flag and wire protocol.
Consolidate UI coverage around the common list, selection, context, and equal-emission behavior.
Remove tests specific to deleted protocol policies. Ablate shared correlation and emission guards;
keep reports under ignored target. Review, archive, push PR 142, and inspect final CI and reviews.
