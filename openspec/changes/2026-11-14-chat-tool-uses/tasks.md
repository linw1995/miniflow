# Tasks

## 1. Revised design

- [x] 1.1 Inspect the actual message/result/preview contracts and identify missing public Agent message outputs.
- [x] 1.2 Plan source authority, scoped identities, late association, multiple attempts, and conflict presentation.
- [x] 1.3 Plan event-driven delivery, related tables, differential writes, and preservation of legacy observation data.
- [x] 1.4 Inspect existing two-slot/role constraints and require canonical message persistence for session restoration.

## 2. Structured messages and source correlation

- [ ] 2.1 Add optional terminal messages and typed append/replay-safe message events to the chat contract/decoder.
- [ ] 2.2 Expose only current-invocation Agent messages, excluding historical/system inputs and the admitted prompt.
- [ ] 2.3 Propagate explicit message/model/tool origin to standard dispatch observations without changing provider arguments.
- [ ] 2.4 Add a shared reducer for semantic calls and independent executions, including unlinked and contradictory facts.

## 3. Relational persistence

- [ ] 3.1 Evolve canonical message storage for multiple assistant/tool rows, empty-text tool calls, complete metadata,
  source keys, and transactional session ordering without a fixed two-message reservation.
- [ ] 3.2 Persist every valid Flow message and complete call/result structure; do not substitute lossy display summaries.
- [ ] 3.3 Restore full successful-turn model history from message relations; keep partial unsuccessful turns inspectable
  without adding them or metrics-only observations to model context.
- [ ] 3.4 Add scoped call/execution/checkpoint FKs, link tools to persisted message rows, and remove live JSON storage.
- [ ] 3.5 Migrate prior draft journals as observation-only records without invented messages, run IDs, or associations.
- [ ] 3.6 Atomically apply message/relation/cursor changes, finalize messages with the turn, and retain complete save-only recovery.
- [ ] 3.7 Update only dirty rows/fields, batch page loads, and prove no payload/sibling write or redundant replay write.

## 4. Event-driven presentation

- [ ] 4.1 Subscribe to receiver lifecycle/value events with a lossless bootstrap and no periodic snapshot polling.
- [ ] 4.2 Push reducer changes to independent bounded/coalescing persistence and frontend consumers.
- [ ] 4.3 Make terminal drawing read-only; render one linked semantic item with its attempts and labeled unlinked observations.
- [ ] 4.4 Preserve source-specific state/content, uncertainty, safe details, retained results, paging, and stale-event isolation.

## 5. Validation and review

- [ ] 5.1 Test messages-only, observations-only, both arrival orders, ID reuse, multiple attempts, and conflict cases.
- [ ] 5.2 Round-trip multiple/empty-text tool messages, full metadata, large valid content, and call counts beyond display
  limits; after restart assert the next provider request contains the complete prior successful conversation.
- [ ] 5.3 Test idle/no-render delivery, reliable message backpressure, cancellation, final draining, and restart without execution.
- [ ] 5.4 Recompile the affected-test Flow and run selected tests, focused coverage, `prek -a`, and strict OpenSpec validation.
- [ ] 5.5 Update PR scope/validation/AI disclosure and verify no conflicts with current main before readiness.

The earlier implementation's passing tests validate its prior behavior only. They do not complete these revised tasks.
