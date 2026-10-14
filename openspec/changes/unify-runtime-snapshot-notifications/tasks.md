## 1. Separate notification from recording policy

- [x] 1.1 Make SnapshotRecorder a stateless event callback and move the old state into caller-owned SnapshotRecording.
- [x] 1.2 Preserve finite snapshot storage/encoding and retain optional stream identities without deduplicating emissions.
- [x] 1.3 Adapt finite runner wiring and existing snapshot consumers/tests; commit the task group.

## 2. Unify runtime capture

- [x] 2.1 Route stream tasks, producers, and events through SnapshotRecorder and remove StreamDataObserver/data_capture.
- [x] 2.2 Adapt stream telemetry and runner wiring; preserve original values, publication semantics, and out-of-lock notifications.
- [x] 2.3 Extend existing integration cases for equal-valued emissions and raw stream snapshots; commit the task group.

## 3. Review and complete

- [ ] 3.1 Review AGENTS.md compliance and ablate unnecessary state, abstraction, and test duplication.
- [ ] 3.2 Run affected coverage, prek, and pinned Nix flake validation; commit final corrections.
- [ ] 3.3 Archive the reviewed specification and update the existing PR; monitor CI and review feedback.
