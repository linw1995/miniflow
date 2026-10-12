# CI tests

Pull requests and pushes to `main` run `scripts/run-cov.sh --affected`, using
`cargo affected run` with the same
workspace, target, feature, and nextest profile selection as the full suite.
The pinned `cargo-affected` version is provided by `nix develop`.
Formatting, package builds, Clippy, and repository hooks still run in CI.

Only a successful Linux full-test collection on the default branch publishes
the coverage database and its Git commit anchor. PRs restore that baseline
without publishing a replacement. The checkout includes full Git history.
Missing baselines, missing Git anchors, and changed build fingerprints fall
back to all tests. Each run uploads `affected-tests`, a JSON selection report.
Collection uses `scripts/run-cov.sh --collect-affected` so its ambient
instrumentation flags match affected runs and do not invalidate the fingerprint.

Coverage-based selection is approximate. Changes to non-Rust files, Rust
fixture inputs, build scripts, and the runtime derive crate force the full
suite. Metadata rules in
`Cargo.toml` select CLI and generated-runner integration tests when their Rust
inputs change, because coverage from the test binary cannot observe every
subprocess. Keep these rules current when adding subprocess test suites or
proc-macro crates. This intentionally limits speedups for compiler and runtime
changes; measured CI timings determine whether finer selection is worthwhile.

## Codecov reports

PR reports use the `rust-affected` flag and only report patch coverage in
checks and comments. Full reports use both `rust` and `rust-affected`: `rust`
provides project coverage, and `rust-affected` seeds the baseline for patch
comparisons. A full-suite fallback in PR CI can therefore also report project
coverage. When only an incremental report is uploaded, the project status
passes without evaluating full-project coverage, preserving its status context;
the patch status evaluates the uploaded coverage. No tests selected means no
coverage report is generated or uploaded.
Incremental reports from `main` are not uploaded, keeping default-branch
coverage history and its badge based on complete reports.

Neither flag uses carryforward. Codecov carries forward a whole missing flag,
not unexecuted tests within a newly uploaded flag. A partial report cannot
replace `rust`, and merging old LCOV hits into a new report could mask genuine
coverage losses. Patch coverage describes the current selected tests; it does
not establish full-suite coverage of unchanged code. See Codecov's
[carryforward flags](https://docs.codecov.com/docs/carryforward-flags) and
[patch-only comments](https://docs.codecov.com/docs/pull-request-comments).

## Scheduled full tests

`Nightly full tests` checks the default branch daily at 18:23 UTC (02:23 in
Asia/Shanghai). It runs all Linux and macOS tests, refreshes the Linux selection
baseline, and generates full coverage reports for Codecov. Linux collects
per-test mappings and aggregate coverage in separate full-suite runs.

Scheduled full tests run when any of these conditions holds:

- The default branch has commits in the last 24 hours.
- The current commit differs from the last successful full suite, including
  changes missed by a delayed schedule or a failed previous run.
- No successful full suite ran in the last seven days.

Skipped, failed, and partial suites do not reset the weekly deadline.
Manual dispatch always runs the full suite. Dispatch on another branch does
not publish a baseline for PRs. After deploying this workflow, dispatch it on
the default branch to seed the first baseline; until then, CI runs all tests.

Local validation remains `nix develop --command prek -a` and
`nix flake check -L`, which always includes the complete test suite.
