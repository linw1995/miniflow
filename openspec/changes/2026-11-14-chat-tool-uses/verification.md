# Verification

## Revised-plan status

The two-source, event-driven, relational redesign in `design.md` is a plan pending implementation. The checks below
record the earlier telemetry-only/polling/JSON baseline. They do not validate the new message contract, source merge,
association tables, differential writes, or event subscriptions. This planning revision changes no runtime code.
Fresh verification is required after the revised tasks are implemented.

## Previous implementation scope

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

After merging `main` at `99c7dc5a`, using the pinned Nix toolchain:

- Recompiled shared test-affected Flow, merge-base comparison against local `origin/main`, test precision, telemetry
  enabled, and `run=true`: 516 passed, one skipped by configuration. The completed report covers the full PR's
  `mf-chat`, `mf-tui`, and `mf-cli` scope without a fallback or graphical inputs.
- Shared storage/controller/terminal libraries: 227 passed. All three executable-chat integration tests passed,
  including the live/restart calculator CLI regression, separately and in the shared affected suite.
- Repository Python suite after excluding desktop-only additions: 76 passed.
- Official coverage script with the affected plan and explicit chat filters: 44 passed; 465 unrelated tests were
  intentionally filtered. The partial report is `target/coverage/result/lcov.info`, not a full-workspace coverage claim.
- Clippy with all targets/features and warnings denied, `prek -a`, strict OpenSpec validation, and the unchanged
  workspace license audit passed.

The two main-merge conflicts were confined to `.config/nextest.toml`. Both the ToolUse calculator test and the
upstream generated-variable test remain in the matching setup and shared-Cargo filter expressions; neither side's
test was dropped. No unmerged entries remain.

Raw logs, immutable plans, profiles, and coverage artifacts stay under ignored `target/`. No machine-specific reports,
application databases, or graphical prototype code are distributed by this PR.
