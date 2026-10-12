# Implementation Tasks

## 1. Crate and Storage Initialization

- [x] 1.1 Add the `mf-chat` library and workspace-managed Diesel, migration, SQLite, and ownership-lock dependencies; verify pinned-toolchain compilation and unique SQLite native linkage.
- [x] 1.2 Add the four-table migration, generated schema, row models, and database constraints; verify fresh initialization, migration rollback/reapply in a disposable database, and reopen without data loss.
- [x] 1.3 Implement `mf_chat.storage.initialize`, connection pragmas, and exclusive host ownership; verify overlapping owners fail before recovery and ownership is released after initialization failure or shutdown.
- [x] 1.4 Embed migrations with build-script directory tracking; verify migration-only changes rebuild the host and initialization succeeds without migration source files.
- [x] 1.5 Document the database path, single-owner boundary, migration behavior, and typed storage errors in `docs/chat.md`; verify documented initialization against a temporary file-backed database.

## 2. Session and Message Nodes

- [x] 2.1 Register typed session create/list/load/rename/archive/restore nodes and stable value contracts; verify node preparation performs no filesystem or database I/O and execution errors preserve sources.
- [x] 2.2 Implement bounded keyset pagination and stable message ordering; verify page boundaries, unknown sessions, archive/restore preservation, and isolation between two sessions.
- [x] 2.3 Implement successful-turn history loading and its configurable byte bound; verify failed turns and the current prompt are excluded, system instructions remain separate, and excess history fails without trimming.
- [x] 2.4 Add host session operations through isolated node targets or embedded maintenance flows; verify every operation uses the configured database and explicit session identity.
- [x] 2.5 Document session operations and archived-session behavior alongside the nodes; verify the examples exercise create, rename, archive, restore, and paginated loading.

## 3. Turn and Model-Call Lifecycle

- [x] 3.1 Implement atomic `turn.begin` with explicit turn/call IDs and per-session running admission; verify concurrent submissions admit only one and leave no database transaction open during provider waiting.
- [x] 3.2 Implement `turn.complete` with atomic assistant message, usage, turn state, and activity updates; verify injected transaction failures leave no partial completion.
- [x] 3.3 Implement idempotent completion replay and fresh-identity model retry with `retry_of`; verify equivalent replay adds no records and conflicting replay fails without changing the saved result.
- [x] 3.4 Implement turn/call failure finalization and owner-protected startup recovery; verify cancelled and abandoned work is excluded from history and late cleanup cannot replace a committed success.
- [x] 3.5 Document turn states, commit retries, model retries, and the remote-call crash boundary; verify failed submissions and their linked retries remain inspectable after reopen.

## 4. Embedded Conversation Flow and Host Lifecycle

- [x] 4.1 Add `conversation.json`, the embedding build helper, module inclusion, registry retention, and selected model-profile preparation; verify invalid preparation creates no running turn and needs no credentials or database access.
- [x] 4.2 Execute the complete conversation flow with fresh contexts and explicit identities; verify two turns against a local mock provider send ordered successful history and append each prompt exactly once.
- [x] 4.3 Add failure/cancellation cleanup with an independent context and typed combined errors; verify provider failure, deadline, cancellation, and cleanup failure expose the correct durable state and IDs.
- [x] 4.4 Return retainable uncommitted completions from validated intermediate results; verify a transient database failure can be retried through the completion task without a second provider request.
- [x] 4.5 Add bounded host execution, foreground capacity errors, and join-before-close behavior; verify separate sessions progress within the limit and shutdown leaves no detached local work.
- [x] 4.6 Document the host API, profile configuration, cancellation, and completion retry path; verify a minimal embedded host runs the documented two-turn example.

## 5. Embedded Session Title Flow

- [x] 5.1 Schedule one pending automatic title call in the first-turn completion transaction; verify replay creates no duplicate job and pending jobs survive reopening.
- [x] 5.2 Add title preparation and completion nodes plus `session_title.json`; verify a separately configured local mock model summarizes the first completed turn without appending chat messages.
- [x] 5.3 Implement title revision checks, validation, and accounting for skipped or rejected application results; verify manual renaming wins both before invocation and during a running title call.
- [x] 5.4 Add pending-title scheduling, failure cleanup, and explicit title retry; verify title failure does not roll back a conversation and interrupted calls are not automatically resent.
- [x] 5.5 Document first-turn title timing, manual-title precedence, and retry behavior; verify saved title outcomes and call usage remain visible independently of turn status.

## 6. Embedded Usage Statistics Flow

- [ ] 6.1 Add typed `mf_chat.usage.query` and `usage_statistics.json`; verify queries run without provider credentials or model requests and use persisted call facts only.
- [ ] 6.2 Implement one-snapshot overall and bounded grouped aggregates with session, configuration, purpose, and UTC interval filters; verify group pagination does not change overall totals.
- [ ] 6.3 Implement nullable subtotals, reported/missing counts, attempted-call status counts, and checked arithmetic; verify sparse counters, explicit zero, empty results, overflow, and exclusion of unstarted title jobs.
- [ ] 6.4 Verify title calls discarded after manual renaming and separate failed retries remain correctly attributed without duplicate accounting.
- [ ] 6.5 Document statistics fields and unknown-consumption semantics; verify examples distinguish known subtotals from complete usage and include both conversation and title purposes.

## 7. Packaging and Integration Validation

- [ ] 7.1 Include embedded JSON, migrations, and integration fixtures in Cargo/Nix sources and update license auditing and documentation links; verify Cargo package contents and source-free execution of all three flows.
- [ ] 7.2 Run a host acceptance scenario spanning two sessions, restart, failed-turn retry, automatic title, manual rename, and usage queries against local mock providers; verify stable identities and persistence invariants across the sequence.
- [ ] 7.3 Review dependency direction, exports, typed error chains, task completeness, and specification coverage; verify no compiler/runtime chat dispatch or terminal UI implementation was introduced.
- [ ] 7.4 Run `nix develop --command prek -a`, `nix flake check -L`, strict OpenSpec validation, and relevant coverage; verify all checks succeed and retain generated reports only under ignored `target/` paths.

## Workflow follow-up

- Archive this change only after implementation and review are complete.
- Plan integration into the existing terminal UI as a separate change using the core's host operations and explicit identities.
