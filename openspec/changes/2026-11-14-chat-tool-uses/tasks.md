# Tasks

## 1. Revised design

- [x] 1.1 Inspect the actual message/result/preview contracts and identify missing public Agent message outputs.
- [x] 1.2 Plan source authority, scoped identities, late association, multiple attempts, and conflict presentation.
- [x] 1.3 Plan event-driven delivery, related tables, differential writes, and preservation of legacy observation data.
- [x] 1.4 Inspect existing two-slot/role constraints and require canonical message persistence for session restoration.

## 2. Structured messages and source correlation

- [x] 2.1 Define complete terminal messages and typed append/replay-safe message events in the chat contract/decoder.
- [x] 2.2 Return the single Agent `messages` log as complete authoritative context; accept complete input messages,
  remove initial-boundary bookkeeping, and generate immutable event identities in the publishing node.
- [x] 2.3 Propagate explicit message/model/tool origin to standard dispatch observations without changing provider arguments.
- [x] 2.4 Add a shared reducer for semantic calls and independent executions, including unlinked and contradictory facts.

## 3. Relational persistence

- [x] 3.1 Evolve canonical message storage for multiple assistant/tool rows, empty-text tool calls, complete metadata,
  source keys, and transactional session ordering without a fixed two-message reservation.
- [x] 3.2 Persist every valid Flow message and complete call/result structure; do not substitute lossy display summaries.
- [x] 3.3 Restore only the latest successful complete context version; allow arbitrary valid history/system rewrites,
  retain older versions and immutable audit events, and exclude failed versions and telemetry from model context.
- [x] 3.4 Add scoped call/execution/checkpoint FKs, link tools to persisted message rows, and remove live JSON storage.
- [x] 3.5 Migrate prior draft journals as observation-only records without invented messages, run IDs, or associations.
- [x] 3.6 Atomically apply message/relation/cursor changes, finalize messages with the turn, and retain complete save-only recovery.
- [x] 3.7 Update only dirty rows/fields, batch page loads, and prove no payload/sibling write or redundant replay write.

## 4. Event-driven presentation

- [x] 4.1 Subscribe to receiver lifecycle/value events with a lossless bootstrap and no periodic snapshot polling.
- [x] 4.2 Push reducer changes to independent bounded/coalescing persistence and frontend consumers.
- [x] 4.3 Make terminal drawing read-only; render one linked semantic item with its attempts and labeled unlinked observations.
- [x] 4.4 Preserve source-specific state/content, uncertainty, safe details, retained results, paging, and stale-event isolation.

## 5. Validation and review

- [x] 5.1 Test messages-only, observations-only, both arrival orders, ID reuse, multiple attempts, and conflict cases.
- [x] 5.2 Round-trip multiple/empty-text tool messages, full metadata, large valid content, and call counts beyond display
  limits; after restart assert the next provider request contains the complete prior successful conversation.
- [x] 5.3 Test idle/no-render delivery, reliable message backpressure, cancellation, final draining, and restart without execution.
- [x] 5.4 Recompile the affected-test Flow and run selected tests, focused coverage, `prek -a`, and strict OpenSpec validation.
- [x] 5.5 Update PR scope/validation/AI disclosure and verify no conflicts with current main before readiness.
- [x] 5.6 Prove custom Flow compaction/reordering/reset survives restart and reaches the next provider request without
  resurrected history/system input; prove immutable payload sharing preserves duplicate positions and sensitivity.

## 6. Single Agent contract

- [x] 6.1 Require complete message input/output for executable and embedded backends; remove split-input bindings,
  text-only settlement and event/audit reconstruction fallbacks.
- [x] 6.2 Simplify the Agent to required message input and migrate chat examples, callers and fixtures to that shape.
- [x] 6.3 Test obsolete-contract rejection, explicit failed message state, storage-only legacy audit preservation,
  complete context replacement and all existing title/preview/recovery behaviors.
- [x] 6.4 Rebuild the planning Flow, run affected tests and focused coverage, rerun hooks/spec checks and update the PR.

The earlier baseline is not evidence for the revised architecture. Final verification below is tracked against the
new message/relation/event implementation, including real restart and next-provider-request checks.
