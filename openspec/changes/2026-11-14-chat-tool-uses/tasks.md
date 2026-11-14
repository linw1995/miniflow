# Tasks

## 1. Revised design

- [x] 1.1 Inspect the actual message/result/preview contracts and identify missing public Agent message outputs.
- [x] 1.2 Plan source authority, scoped identities, late association, multiple attempts, and conflict presentation.
- [x] 1.3 Plan event-driven delivery, related tables, differential writes, and preservation of legacy observation data.
- [x] 1.4 Inspect existing two-slot/role constraints and require canonical message persistence for session restoration.

## 2. Structured messages and source correlation

- [x] 2.1 Add optional terminal messages and typed append/replay-safe message events to the chat contract/decoder.
- [x] 2.2 Reuse the single Agent `messages` log; derive current-invocation output/events from its immutable initial
  boundary, excluding historical/system inputs and the admitted prompt without a parallel transcript accumulator.
- [x] 2.3 Propagate explicit message/model/tool origin to standard dispatch observations without changing provider arguments.
- [x] 2.4 Add a shared reducer for semantic calls and independent executions, including unlinked and contradictory facts.

## 3. Relational persistence

- [x] 3.1 Evolve canonical message storage for multiple assistant/tool rows, empty-text tool calls, complete metadata,
  source keys, and transactional session ordering without a fixed two-message reservation.
- [x] 3.2 Persist every valid Flow message and complete call/result structure; do not substitute lossy display summaries.
- [x] 3.3 Restore full successful-turn model history from message relations; keep partial unsuccessful turns inspectable
  without adding them or metrics-only observations to model context.
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

The earlier baseline is not evidence for the revised architecture. Final verification below is tracked against the
new message/relation/event implementation, including real restart and next-provider-request checks.
