# Verification

- Reviewed the existing public module surface before replacing the unreleased boolean verbosity fields with counts.
- Pinned formatting, pre-commit, and commit-message hooks installed with `nix develop --command prek install`.
- `nix develop --command prek -a`: all hooks passed.
- Standalone parser: five tests passed, including clusters, repeated flags, capped counts, consumed option-like
  values, and existing transport/schema errors.
- CLI parser: nine tests passed. Real CLI argument/stdio/exit forwarding: five tests passed, including cumulative
  counts before and after a subcommand and capped child arguments.
- Diagnostics: five tests passed, including detailed readable metadata, full-record round trips with trace context,
  escaped control characters, full unsigned stream IDs, interval reminders, and termination cleanup.
- Nine selected ablation executions demonstrated count-cap, cross-subcommand aggregation, and full-trace-record
  regressions, showed that removing their matching tests hides them, and restored all source mutations.
- Actual generated no-SDK task, Loop, and stream acceptance passed for the three selected verbosity levels with
  unchanged result stdout. Existing configured OTLP success/failure acceptance passed at the higher levels.
- Existing TUI integration passed with highest-verbosity forwarding, valid parameters, source failure, interruption,
  and terminal restoration.
- Actual no-SDK examples printed all three requested presentations while preserving `{"answer":42}` on stdout.
- A real 62-second no-SDK streaming node printed four complete lifecycle JSON records and one readable reminder at
  60 seconds under `-vvv`, then completed successfully with unchanged `{"result":"ready"}` stdout.
- Repository coverage script: all 24 selected parser/CLI/diagnostics tests passed. Reports remain under
  `target/coverage/result/`, including `lcov.info`.
- `openspec validate add-verbosity-levels --strict`: passed without findings.

Experimental scripts, report notes, generated examples, coverage, and raw logs remain under ignored `target/` and are
excluded from commits.

- `nix flake check -L`: all native aarch64-darwin checks passed; 543 tests passed, two skipped.
- Final repository-rule and ablation review found no blocking design or implementation issue.

- Change archived with one added and two modified requirements synchronized into the main specification.
- Main specs pass standard validation; strict findings match the existing 25 length warnings exactly.
- The new archive validates without findings. The existing all-archive failure in `2026-10-05-unify-flow-runtime`
  (17 incomplete tasks) remains unchanged.
