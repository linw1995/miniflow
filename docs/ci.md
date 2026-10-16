# CI tests

Pull requests use rust-analyzer's crate graph and Cargo metadata to select
changed packages and their transitive reverse dependencies. The coverage job
checks out the PR head with comparison history, computes changes from the merge
base, and records the selection and reasons in its job summary. It runs those
package suites with coverage; an empty selection skips the coverage cache,
tests, and uploads. Dependency configuration, shared inputs, or analyzer failures
conservatively select the full workspace.

Pushes to `main` and manual CI dispatches run the complete Linux suite through
`nix develop --command bash scripts/run-cov.sh`. Both full and selected runs use
the pinned toolchain, instrument workspace binaries, and write reports to
`target/coverage/result/`, including `lcov.info`. Test failures stop report
generation and fail CI.

Formatting, package builds, Clippy, repository hooks, and CI script tests
also run in CI. Main additionally builds and checks the macOS package;
complete macOS test execution runs in nightly.

Coverage dependencies and generated-project builds use persistent caches.
The script removes previous workspace binaries and report data while retaining
dependencies, so stale coverage mappings cannot contribute to a new report.
Caches accelerate compilation without changing which tests execute.

## Codecov reports

Main, nightly, and full-workspace PR fallback runs upload LCOV with the `rust`
flag. Selected PR runs use `rust-affected`. Automatic report discovery and
carryforward are disabled; reports describe tests executed for the current
source tree.

Project coverage considers only full `rust` reports, and is excluded when that
flag is not uploaded. PR patch coverage accepts either flag. Codecov comments
hide project-wide coverage so a partial run is not presented as a complete
project measurement. See Codecov's [flag status rules](https://docs.codecov.com/docs/commit-status#flags).

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

Run `nix develop --command prek -a` and the tests affected by the change.
Do not run `nix flake check -L` locally: its test check repeats the complete
suite already run by CI coverage.

### Affected package tests

The selector uses rust-analyzer's `rust-analyzer/viewCrateGraph` LSP request
to discover workspace target dependencies, maps targets to Cargo packages,
and selects changed packages plus their transitive reverse dependencies.
It selects whole package suites, including integration tests, rather than
individual test functions. PR CI uses this selector; main and nightly retain
full-suite runs.

Run from the checkout using the pinned tools:

```bash
# Inspect staged, unstaged, and untracked changes against HEAD.
nix develop --command python3 scripts/affected_tests.py --worktree

# Analyze the checked-out commit against its first parent.
nix develop --command python3 scripts/affected_tests.py --commit HEAD

# Analyze a checked-out PR head against its merge base with a local base ref.
nix develop --command python3 scripts/affected_tests.py --base origin/main --head HEAD --json

# Execute the selected package suites, propagating nextest's exit status.
nix develop --command python3 scripts/affected_tests.py --worktree --run
```

Commit and PR modes require a clean checkout at the requested head. The caller
must provide the base and head commits locally; the script does not fetch refs,
query GitHub, or switch branches. CI callers need sufficient checkout history
for `git merge-base`. A root commit compares against an empty tree; merge commits
compare against their first parent. Renames include both old and new paths.

`--json` produces a plan with `packages`, per-package `reasons`, `dependencies`,
`changed_files`, `fallback_reasons`, and a `command` argument array. An empty
selection has an empty command: callers must skip execution instead of invoking
nextest without package arguments. `--run` handles that case automatically.
`--target <triple>` applies to both rust-analyzer and the resulting nextest command.
The selector's `--run` command uses `--locked --all-targets --all-features`.
Coverage retains its existing target selection and changes only the package scope.

To reproduce PR coverage, save the plan outside the checkout and pass it to
the coverage script:

```bash
nix develop --command python3 scripts/affected_tests.py --base origin/main --json > /tmp/affected-tests.json
nix develop --command bash scripts/run-cov.sh --affected-plan /tmp/affected-tests.json
```

`--affected-plan` must be the first coverage-script argument. It replaces the
default workspace selection with explicit package arguments. Empty plans clear
stale reports and exit without running Cargo; malformed plans fail. Other nextest
arguments can follow the plan path.

Cargo metadata supplies package paths, target ownership, and declared local
dependencies. The analyzer graph includes test and build-script targets;
declarations disambiguate duplicate target names. All declared workspace
dependency kinds, including optional and platform-specific dependencies, are
retained even when rust-analyzer's current target configuration omits them.
Such additions appear in `supplemented_edges`. This favors extra tests over
missing a consumer on a different platform. It does not replace testing on
that platform or with other feature combinations.

Files within a package, including fixtures, migrations, and deleted sources,
select that package. Dependency manifests or lockfiles, shared inputs such as
`examples/`, and unclassified paths select the entire workspace. This also
covers removed packages and dependency edges that no longer exist in the head.
Changes confined to `docs/`, `openspec/`, or the root README, contributing guide,
and agent instructions do not select Rust tests. Build scripts or generated
runners can consume inputs beyond Cargo's declared graph, so this is a
conservative package-level selector, not a proof of semantic independence.

The analyzer loads all features with compilation checks, build-script execution,
proc-macro expansion, and cache priming disabled. It must finish loading with a
healthy status before the graph is used. A missing analyzer, timeout (120 seconds
by default, configurable with `--timeout`), incomplete graph, or unsupported
graph format selects all workspace packages with an explicit fallback reason.
Invalid Git refs, a mismatched checkout, and failed Cargo metadata stop with an
error rather than emitting an unreliable plan.

For repeated local coverage runs, reuse a generated-project build cache:

```bash
nix develop
export MF_TEST_BUILD_CACHE_DIR="$PWD/target/nextest-build-cache/local"
bash scripts/run-cov.sh
```

The coverage script forwards additional arguments to `cargo nextest run`,
so explicit test filters remain available for focused local checks. These
checks supplement the required hooks and affected tests.
