# Verification

## Scope

Only ToolUse capture, persistence, terminal presentation, and their tests/documentation are included. Desktop
experiments and their separate license/resource review remain outside this change in ignored local storage.
The license policy, notice generator/template, default dependencies, and root lockfile match the comparison base.

## Functional checks

- Terminal unit tests cover all phases, explicit uncertainty and missing legacy journals, once-per-turn display,
  assistant-only pages, cancelled user-only history, repeated names, literal/control-safe details, retained save-pending
  evidence, stale completion rejection, and previous records surviving a new turn or session change.
- The CLI calculator regression runs the actual compiled Flow against an HTTP test provider. One call executes the
  calculator and two are rejected. A real pseudo-terminal sees tool status/details before a deliberately delayed final
  reply with the Flow panel hidden. Restarting terminal chat restores one journal without additional provider calls.
  The test also checks terminal restoration, empty CLI stdout, and absence of the synthetic credential on screen.
- Storage/controller tests cover bounded redaction, migration/reopen, identity isolation, interruption, final-journal
  immutability, write contention, rollback, and save-only recovery without another execution.

## Validation

Using the pinned Nix toolchain:

- Shared test-affected Flow, worktree mode, test precision, telemetry enabled, and `run=true`: 392 passed, zero skipped,
  with a completed execution report selecting `mf-tui` and its CLI consumer.
- Focused terminal library suite: 177 passed. The live/restart calculator CLI regression passed separately and in the
  shared affected suite.
- Repository Python suite after excluding desktop-only additions: 76 passed.
- Official coverage script with the affected plan and explicit chat filters: 44 passed; 340 unrelated tests were
  intentionally filtered. The partial report is `target/coverage/result/lcov.info`, not a full-workspace coverage claim.
- Clippy with all targets/features and warnings denied, `prek -a`, strict OpenSpec validation, and the unchanged
  workspace license audit passed.

Raw logs, immutable plans, profiles, and coverage artifacts stay under ignored `target/`. No machine-specific reports,
application databases, or graphical prototype code are distributed by this PR.
