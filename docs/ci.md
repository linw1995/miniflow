# CI tests

Pull requests and pushes to `main` run the complete Linux test suite and
coverage through `nix develop --command bash scripts/run-cov.sh`. The script
uses the pinned toolchain, instruments workspace binaries, and writes reports
to `target/coverage/result/`, including `lcov.info`. Test failures stop report
generation and fail CI.

Formatting, package builds, Clippy, repository hooks, and CI script tests
also run in CI. Main additionally builds and checks the macOS package;
complete macOS test execution runs in nightly.

Coverage dependencies and generated-project builds use persistent caches.
The script removes previous workspace binaries and report data while retaining
dependencies, so stale coverage mappings cannot contribute to a new report.
Caches accelerate compilation without changing which tests execute.

## Codecov reports

PR, main, and nightly coverage runs upload the complete generated LCOV report
with the `rust` flag and automatic report discovery disabled. Codecov uses
that flag for project and PR patch coverage. Carryforward is disabled;
reports describe tests executed for the current source tree.

## Scheduled full tests

`Nightly full tests` checks the default branch daily at 18:23 UTC (02:23 in
Asia/Shanghai). It runs all Linux and macOS flake checks and generates a
complete Linux coverage report for Codecov.

Scheduled full tests run when any of these conditions holds:

- The default branch has commits in the last 24 hours.
- The current commit differs from the last successful full suite, including
  changes missed by a delayed schedule or a failed previous run.
- No successful full suite ran in the last seven days.

Skipped, failed, and partial suites do not reset the weekly deadline.
Manual dispatch always runs the full suite.

## Local validation

Required validation remains `nix develop --command prek -a` and
`nix flake check -L`, which includes the complete test suite.

For repeated local coverage runs, reuse a generated-project build cache:

```bash
nix develop
export MF_TEST_BUILD_CACHE_DIR="$PWD/target/nextest-build-cache/local"
bash scripts/run-cov.sh
```

The coverage script forwards additional arguments to `cargo nextest run`,
so explicit test filters remain available for focused local checks. These
checks do not replace required final validation.
