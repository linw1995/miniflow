# Verification

## Implementation scope

The revised message-first, event-driven, relational implementation replaces the earlier telemetry-only/polling/JSON
draft. This change contains only ToolUse, the canonical message storage needed for recovery, terminal presentation,
and associated contracts/tests/documentation. Desktop prototypes remain ignored local material. License policy,
notice generation, default dependencies and the root lockfile are unchanged.

## Functional evidence

- The Agent has one evolving `messages` transcript, not a parallel public-message accumulator. An immutable initial
  count excludes system/history/the admitted prompt from returned messages and zero-based event offsets. Regression
  cases cover identical historical/new text, optional system input, multiple tool rounds, a first/later provider
  failure, the final assistant append, and the eight-round limit; streamed batches reconcile to the terminal suffix.
- Canonical message tests round-trip empty-text tool-call assistants, multiple assistant/tool messages, complete
  metadata/arguments/results, optional empty tool-call arrays, large Unicode payloads and 130-call conversations.
  Successful history reconstruction crosses pagination boundaries; unsuccessful prefixes remain inspectable but do
  not enter the next request's model context. Native sensitivity survives reopen without entering public previews.
- SQL rejection triggers prove status-only updates do not touch payloads, sibling executions or model-call rows.
  Exact replay writes no duplicate messages, order/cursor changes or execution payloads. Final-save failures roll
  back settlement and preserve full save-only recovery. Legacy JSON migrates as observation-only data.
- Source tests exercise both arrival orders, late origin, reused provider IDs, multiple attempts, conflicts, missing
  proof, and late ambiguity. Ambiguity removes the association without deleting messages or execution facts.
- Receiver subscription tests bootstrap already-admitted state, receive live changes, drain closure, detach, and
  attach after closure without rendering or querying observation snapshots. Existing cancellation/backpressure and
  paused-rendering regressions remain applicable; ToolUse drawing no longer collects or persists observations.
- The actual compiled calculator Flow produces one successful and two rejected tools. A real pseudo-terminal shows
  their facts before a delayed final reply with the Flow panel hidden. Restart restores the message/call/result rows
  without execution; continuing that session sends its full prior assistant tool calls/results to the provider.
- A second real-provider regression reuses a provider tool-call ID across two model rounds. Both executions retain
  distinct identity, correct 10/21 results and their own model-scoped links after reopening. Public origin is produced
  independently of prior business-message sensitivity, not recovered from masked snapshots or guessed by name.
- Literal terminal formatting, explicit unobserved/unlinked states, multiple attempts, stale invocations and legacy
  missing evidence remain covered. CLI stdout stays empty and terminal modes are restored.

## Final validation

Using the pinned Nix Rust 1.99.0 toolchain:

- Recompiled the shared planning Flow and ran the immutable worktree selection with test precision, telemetry enabled
  and `run=true`: **1263 passed, 5 skipped** across 143 binaries. Six affected packages were selected with no fallback.
  The successful single-transcript source snapshot is `35f456d10f6633c7bc1bd1544289cfd78cc6c5ed`; all four CLI chat
  integration tests are included. `target/desktop-probe/single-messages-test-plan.json` retains the executable plan
  and execution report. An initial attempt exhausted shared disk space; only generated local caches were cleaned
  before rerunning the complete selection successfully.
- Repository Python checks: **76 passed**.
- Official coverage script with the affected plan and explicit test-name filters: **44 passed, 1215 filtered**.
  `target/coverage/result/lcov.info` is a **partial** focused report, not full-workspace or complete-crate coverage.
- `prek -a` covers formatting, all-target/all-feature Clippy and Cargo check, Markdown, workflow validation and the
  unchanged license audit. Strict OpenSpec validation is run for this change before submission.

Updated old graph/output-shape and history-ID assertions reflect the new optional ports and per-assistant ID scope;
within-message duplicate IDs and unpaired tool results still fail before HTTP. No test was removed to hide a failure.

## CI status

The preceding implementation commit `42d17826` passed CI lint and Ubuntu checks, but its cumulative incremental
coverage was 2255/2421 lines (93.14%), below the 95% gate. That failure is not presented as passing validation, and
this single-transcript refactor does not weaken the gate. CI for the updated PR head must be evaluated separately.

Raw logs, plans and coverage reports remain under ignored `target/`. No application database, machine-specific
report, desktop code or graphical licensing material is included in the PR.
