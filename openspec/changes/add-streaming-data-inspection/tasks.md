## 1. Protocol and bounded export

- [x] 1.1 Collect a clean local affected-test baseline with the pinned toolchain before Rust edits; verify `target/affected/coverage.db` and `baseline.sha` match the local clean commit and keep generated files under `target/`.
- [x] 1.2 Define the independently versioned stream data envelope, record identities, inline typed-value codec, and capped encoding; verify round trips for unsigned numbers, integer/float distinctions, nested values, omitted oversized/deep values, and checked counter exhaustion.
- [x] 1.3 Implement nonblocking count/byte-bounded data export with start/status/end evidence and typed diagnostics; verify queue pressure, missing endpoints, failed delivery, cumulative loss accounting, and finite shutdown deadlines without synchronous flushes in runtime locks.
- [x] 1.4 Document the new protocol and capture budgets in `docs/observability.md`; verify the documented wire fields agree with codec tests and unchanged finite snapshot/lifecycle versions.

## 2. Runtime capture boundaries

- [x] 2.1 Add the optional runtime-owned recorder to streaming options and propagate capture through execution contexts using existing stream identities; verify invalid observation pairing fails before startup and disabled capture creates no recorder or payload serialization.
- [x] 2.2 Capture synchronous task inputs and validated/published outputs, including Loop and parallel Iteration bodies; verify distinct invocation/span association and value equality with capture disabled, including dynamic fallback where required.
- [x] 2.3 Capture producer inputs and each successfully admitted send with output message identity and ordinal; verify live delivery before producer completion, several emissions, rejected sends, failure after a valid send, cancellation, and unchanged business backpressure.
- [x] 2.4 Capture event input ports and each admitted emission for input, startup, timer, and upstream-close triggers; verify zero-emission Batch buffering, timeout/close flushes, partial publication failures, and absence of fabricated input messages.
- [x] 2.5 Verify bounded handoff reserves identities at admission and performs encoding/export outside coordinator locks using focused concurrent runtime tests; document the programmatic setup and lock-boundary contract with a working example.

## 3. Runner and TUI launch configuration

- [x] 3.1 Add `MF_CAPTURE_STREAM_DATA` handling and generated runner exporter/recorder setup; verify telemetry-enabled capture, no-telemetry/missing-export diagnostics, ordinary execution without capture, and unchanged whole-run snapshot rejection.
- [x] 3.2 Configure TUI child environments by execution mode without changing the parent; verify stream data enablement, inherited finite snapshot suppression, finite capture, endpoint isolation, and metadata-only behavior for older compatible runners.
- [x] 3.3 Add generated-runner acceptance coverage comparing streaming results/order with capture enabled and disabled; verify producer, Batch, and nested task records agree with in-memory execution, and selected finite Tasks from stream binaries retain finite snapshot semantics.
- [x] 3.4 Update runner/TUI capture documentation and examples in `docs/observability.md`; verify examples against the generated-runner tests and state the recompilation and large-value omission behavior.

## 4. Receiver and bounded data store

- [ ] 4.1 Admit dedicated stream data records before generic raw-log preview truncation, validate session/version/context, and decode independently; verify out-of-order records, unrelated runs, missing prior records, sampled/missing spans, and typed malformed-record diagnostics.
- [ ] 4.2 Implement count/byte-bounded payload storage and indexes with eviction, bounded sequence witnesses/gap accounting, and retransmission handling; verify old values/indexes are released, conflicting duplicates are diagnosed, and ancient records cannot repopulate history.
- [ ] 4.3 Expose invocation-scoped/session data views and separate capture availability, omissions, sender loss, transport gaps, local eviction, and faults; verify missing start/end evidence, overlapping loss accounting, unsupported versions, and continued lifecycle/raw-log/finite-snapshot ingestion after Data faults.
- [ ] 4.4 Document receiver limits and diagnostics in `docs/observability.md`; verify sustained synthetic admission remains inside the stated memory budgets including indexes and decoded values.

## 5. Data list presentation

- [ ] 5.1 Render span-correlated and session-wide stream Data lists with row summaries and selected value/omission details; verify multi-emission producers, repeated nested identities, zero-output callbacks, and live session data before a span arrives.
- [ ] 5.2 Add record-identity selection, paging, tail following/resume, and explicit selection-eviction handling using existing TUI key conventions; verify insertions/reordering do not move an inspected selection and eviction releases its value.
- [ ] 5.3 Preserve mutable Data refresh, bounded UTF-8/control-safe previews, completed inspection, and finite Data behavior; verify late records/status changes and collecting-to-finished transitions with rendering tests.
- [ ] 5.4 Document Data scope/navigation and diagnostic meanings in `docs/observability.md`; verify documented keys against the implemented handlers and rendered help.

## 6. Integration and final validation

- [ ] 6.1 Run end-to-end stream observation with queue saturation, retention eviction, interrupted producers, failed export, and omitted large values; verify retained data remains inspectable and business results and lifecycle meanings remain unchanged. Run affected coverage during implementation and inspect `target/affected/report.json` for selection/fallback behavior.
- [ ] 6.2 Run `openspec validate add-streaming-data-inspection --strict`, `nix develop --command prek -a`, and `nix flake check -L`; verify all required checks pass and review implementation against both spec deltas.

## Workflow follow-up

- Archive this change only after implementation and review are complete; keep local coverage/measurement artifacts under Git-ignored `target/`.
