# Verification

## Implementation scope

This revision implements the review-requested complete-context contract in addition to message-first ToolUse,
event-driven collection, relational persistence and terminal presentation. The Agent has no persistence boundary
counter or parallel public-message accumulator. Desktop prototypes and raw validation artifacts remain ignored local
material. License policy and notice generation are unchanged. The only dependency-edge change reuses the already
pinned workspace SHA-256 implementation for immutable payload interning; no dependency version is updated.

## Functional evidence

- The standard Agent consumes complete `messages` when supplied, otherwise retaining legacy prompt/system/history
  assembly. It appends each assistant/tool result once and returns its complete log, including the final reply.
  Tests cover equal historical/new text, optional system input, multiple rounds, provider failure and the eight-round
  limit. The Flow maintains neither host journal offsets nor a message-prefix count.
- A real custom compiled Flow accepts complete messages, invokes an HTTP provider, and returns a compacted context
  with replacement system instructions. After database reopen, the next actual provider request contains exactly
  that version plus its new user message, without the removed system instruction or old prompt. A subsequent
  replacement drops the preceding answer, while the older context version remains available by invocation.
- Context-version tests edit and reorder messages, merge history into a summary, remove prompts and system messages,
  reset to a single reply, retain repeated equal message positions and append later legacy text-only turns. Only the
  most recent successful version controls model history. Failed versions and failed settlements cannot advance it.
- Context payloads and ordered memberships are relational, not a whole-transcript JSON column. Equal payloads may
  share storage without merging message identities or supplying execution provenance. SQL triggers prove immutable
  payload reuse, zero-write exact result replay and atomic rollback. Scoped ownership and fail-fast contention are
  exercised explicitly.
- Full metadata/arguments/results, optional empty tool-call arrays, empty-text tool-call assistants, large Unicode
  values and 130-call contexts survive reopen independently of display limits. Sensitive and public values with the
  same business JSON do not share a payload key. Native sensitivity survives restoration and public previews remain
  bounded/redacted.
- Immutable publisher-generated message events are independent of the final context. A compacted terminal state
  can remove a streamed tool exchange without deleting its original request/result or explicit execution links.
  Failed event checkpoints retain the original event identity and full batch in the reducer; Save-only recovery
  persists that identity, and later native-event replay performs no writes. No model/tool rerun is involved.
- Source tests retain both arrival orders, late correlation, repeated provider IDs, multiple attempts, conflicts and
  ambiguity. Only explicit invocation/model/provider-tool identity links executions. Context-only tool projections
  have no invented origin or execution links and are labeled `context` in the terminal.
- Receiver tests retain bootstrap/live handoff, closure draining, detachment and no-render delivery. Drawing remains
  read-only and no timed snapshot polling was reintroduced. Canonical checkpoints use reliable output-consumer
  backpressure; observation/frontend subscribers remain independent of drawing.
- Actual calculator + HTTP-provider + PTY regressions show one successful and two rejected tools before delayed final
  text with the Flow panel hidden. Restart restores their facts without execution. Continuing the session sends the
  complete returned context, including tool calls/results. Another real test reuses provider tool IDs across model
  rounds while retaining distinct 10/21 results and model-scoped execution links. Stdout and terminal restoration
  assertions remain enabled.

## Final validation

Using the pinned Nix Rust 1.99.0 toolchain:

- Shared planning Flow recompiled after implementation changes; immutable worktree planning at test precision with
  telemetry and `run=true`: **1274 passed, 5 skipped** across 143 binaries, six affected packages, no fallback.
  Source snapshot: `371235a7ff28ae8127728e109bd676fb16a6d0e4`.
  `target/desktop-probe/authoritative-state-test-plan.json` retains its executable plan and execution report.
- Focused library/Agent/custom-provider/PTY regressions: **383 passed**. The full selection above includes them and
  the documented executable examples. Old delta-only output expectations were updated to assert full contexts,
  including their original system/history inputs; no test was removed to hide a failure.
- Repository Python checks: **76 passed**.
- Official coverage script with the affected plan and explicit test-name filters: **55 passed, 1215 filtered**.
  `target/coverage/result/lcov.info` is a partial focused report, not a full-workspace or complete-crate coverage claim.
- `prek -a` includes formatting, all-target/all-feature Clippy and Cargo check, Markdown, workflow validation and the
  unchanged license audit. Strict OpenSpec validation is required before submission.

## CI status

The previous head `b81519ae` passed lint and Ubuntu checks, but its cumulative incremental coverage gate failed:
2295/2462 lines (93.22%), below the required 95%. That failure is not presented as passing validation. This revision
does not weaken the gate; the updated PR head requires a fresh CI result and the PR remains Draft.

Raw logs, plans and coverage reports stay under ignored `target/`. No application database, machine-specific report,
desktop code or graphical licensing material is included in the PR.
