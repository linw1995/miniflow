# Verification

## Implementation scope

The revised message-first, event-driven, relational implementation replaces the earlier telemetry-only/polling/JSON
draft. This change contains only ToolUse, the canonical message storage needed for recovery, terminal presentation,
and associated contracts/tests/documentation. Desktop prototypes remain ignored local material. License policy,
notice generation, default dependencies and the root lockfile are unchanged.

## Functional evidence

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
  and `run=true`: **1260 passed, 5 skipped** across 143 binaries. Six affected packages were selected with no fallback.
  The successful source snapshot is `0d0536bfeafae465d6ed24036d9af4bd03ba5e63`; all four CLI chat integration tests are
  included. `target/test-plan.json` retains the executable plan and execution report.
- Repository Python checks: **76 passed**.
- Official coverage script with the affected plan and explicit test-name filters: **39 passed, 1217 filtered**.
  `target/coverage/result/lcov.info` is a **partial** focused report, not full-workspace or complete-crate coverage.
  A prior overbroad instrumented attempt hit the command budget; its profiles are excluded from this regenerated report.
- `prek -a` covers formatting, all-target/all-feature Clippy and Cargo check, Markdown, workflow validation and the
  unchanged license audit. Strict OpenSpec validation is run for this change before submission.

Updated old graph/output-shape and history-ID assertions reflect the new optional ports and per-assistant ID scope;
within-message duplicate IDs and unpaired tool results still fail before HTTP. No test was removed to hide a failure.

Raw logs, plans and coverage reports remain under ignored `target/`. No application database, machine-specific
report, desktop code or graphical licensing material is included in the PR.
