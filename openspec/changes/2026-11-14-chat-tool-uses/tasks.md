# Tasks

## 1. Revised design

- [x] 1.1 Inspect the actual message/result/preview contracts and identify missing public Agent message outputs.
- [x] 1.2 Plan source authority, scoped identities, late association, multiple attempts, and conflict presentation.
- [x] 1.3 Plan event-driven delivery, related tables, differential writes, and preservation of legacy observation data.

## 2. Structured messages and source correlation

- [ ] 2.1 Add optional terminal messages and typed append/replay-safe message events to the chat contract/decoder.
- [ ] 2.2 Expose only current-invocation Agent messages, excluding historical/system inputs and the admitted prompt.
- [ ] 2.3 Propagate explicit message/model/tool origin to standard dispatch observations without changing provider arguments.
- [ ] 2.4 Add a shared reducer for semantic calls and independent executions, including unlinked and contradictory facts.

## 3. Relational persistence

- [ ] 3.1 Add scoped FK/unique-key tables for semantic calls, executions, and small source checkpoints; remove live JSON storage.
- [ ] 3.2 Migrate prior draft journals as observation-only records without invented messages, run IDs, or associations.
- [ ] 3.3 Apply only dirty fields/rows, batch page loads, retain failed/cancelled facts, and preserve save-only recovery.
- [ ] 3.4 Prove no payload/sibling rewrite on status changes and no write on exact replay; test migration and ownership.

## 4. Event-driven presentation

- [ ] 4.1 Subscribe to receiver lifecycle/value events with a lossless bootstrap and no periodic snapshot polling.
- [ ] 4.2 Push reducer changes to independent bounded/coalescing persistence and frontend consumers.
- [ ] 4.3 Make terminal drawing read-only; render one linked semantic item with its attempts and labeled unlinked observations.
- [ ] 4.4 Preserve source-specific state/content, uncertainty, safe details, retained results, paging, and stale-event isolation.

## 5. Validation and review

- [ ] 5.1 Test messages-only, observations-only, both arrival orders, ID reuse, multiple attempts, and conflict cases.
- [ ] 5.2 Test idle/no-render delivery, backpressure, cancellation, final draining, and real terminal restart without execution.
- [ ] 5.3 Recompile the affected-test Flow and run selected tests, focused coverage, `prek -a`, and strict OpenSpec validation.
- [ ] 5.4 Update PR scope/validation/AI disclosure and verify no conflicts with current main before readiness.

The earlier implementation's passing tests validate its prior behavior only. They do not complete these revised tasks.
