# CI tests

Pull requests use rust-analyzer's crate graph and Cargo metadata to select
changed packages and their transitive reverse dependencies. The coverage job
checks out GitHub's PR merge commit with comparison history and compares it
with the event's base commit. Both dependency analysis and tests therefore
include changes already on the base branch. The job records the selection and
reasons in its summary. It runs those
package suites with coverage; an empty selection skips the coverage cache,
tests, and uploads. Dependency configuration, shared inputs, or analyzer failures
conservatively select the full workspace.

Pushes to `main` and manual CI dispatches run the complete Linux suite through
the same coverage script as local runs. Both full and selected runs use
the pinned toolchain, instrument workspace binaries, and write reports to
`target/coverage/result/`, including `lcov.info`. Test failures stop report
generation and fail CI.

Formatting, package builds, Clippy, repository hooks, and CI script tests
also run in CI. Main additionally builds and checks the macOS package;
complete macOS test execution runs in nightly.

Coverage dependencies and generated-project builds use persistent caches.
The script removes previous workspace binaries and report data while retaining
dependencies, so stale coverage mappings cannot contribute to a new report.
The packaged CLI fixture keeps a separate target beside the generated-runner
cache. Its registry dependencies survive subsequent runs, while preparation
cleans checkout packages before rebuilding the CLI. Packaged source digests also
invalidate registry builds of the support packages and external fixture whenever
their build inputs change, even at the same version. Cargo's generated VCS
metadata is excluded from this identity because these crates do not consume it;
metadata-only repackaging retains compiled dependencies. Crate-local packaged
lockfiles are excluded too: dependency builds use the consumer's lockfile, and
packaged lockfiles contain revision-dependent checksums. Cargo's archive and file
checksums still cover every packaged file. Workspace cleanup excludes these
content-validated packages: Cargo otherwise removes their registry fingerprints
by package name even when it retains their libraries. Content invalidation runs
before both CLI and runner compilation. Dependency artifacts are
seeded only on the first run; installed executables, Cargo homes, and user
projects remain temporary and isolated.
The default and CI profiles share the generated-build fixtures for the CLI,
compiler, and chat host. The chat fixture copies only its own build inputs and
keeps dependency paths in the checkout; it deletes the copied workspace before
running the relocated host. All shared builds use one serialized test group.
Only release dependencies are prewarmed; obsolete dev artifacts are removed.
Generated-runner behavior tests use the release profile with optimization level
zero by default. This keeps release semantics and artifact paths while reducing
compilation cost. Set `CARGO_PROFILE_RELEASE_OPT_LEVEL` to override the default;
the dedicated LTO/strip test explicitly requests level three. Existing Rust flags,
including coverage instrumentation, are preserved.
After reporting, CI runs `scripts/run-cov.sh --prune-cache` before saving the
cache. It removes checkout artifacts that the next preparation would delete and
executables belonging to temporary generated projects. Registry libraries,
proc macros, and dependency build scripts remain cached. Reports are preserved;
run `--prepare` again before executing more package steps after pruning.
Caches accelerate compilation without changing which tests execute.

## Coverage step timings

CI and nightly prepare and build the selected packages together, run a separate
`Coverage tests (<package>)` step for each workspace package, and generate one
combined report after all selected suites pass. The shared build preserves Cargo
feature unification and is timed separately. Package steps reuse nextest's binary
and Cargo metadata, including only test discovery, setup, and execution. Generated
project builds inside tests still count toward their package's time, and their
caches are shared across steps. Generated-project dependencies are cleaned and
prewarmed once per coverage preparation session, when the first matching package
runs. Later package steps reuse that preparation; a new `--prepare` invocation
creates a new session and invalidates the marker. Package steps must run
sequentially against the same checkout and prepared binaries. New workspace
packages must also be added to both workflows; the CI script tests check that every package has exactly one step.

On PRs, step-level conditions skip packages absent from the affected plan.
An empty plan skips preparation, all package steps, report generation, and uploads.
After each non-cancelled PR CI run, the timing monitor creates or updates one PR
comment. Its Summary lists skipped crates, total CI wall time, coverage job time,
the sum of executed crate-step durations, and each crate's result and duration.
Skipped crates show `N/A`; missing timestamps also produce `N/A` rather than a
partial total. Shared compilation, report generation, uploads, and other steps
remain available in the detailed timing comparison. The Summary uses GitHub's
second-resolution timestamps, so a successful `0s` step is distinct from a skip.
Failed runs can skip crates because an earlier step failed, not just because
the affected plan excluded them.

The monitor omits skipped steps from timing comparisons and computes each step's baseline median
from successful executions of that exact step name only. Skipped, missing, and
failed historical steps never contribute zero-duration samples. A step without
successful baseline samples shows `N/A`. Job and wall times still reflect the
work actually selected, so runs with different selections are not equivalent
full-suite measurements.

The script's `--prepare`, `--test-package <name>`, and `--report` modes support
these separate steps. Preparation accepts `--affected-plan <path>`, clears stale
profiles and workspace binaries, and builds only the selected package suites.
Package steps accumulate profiles, and reporting merges them. Local invocations
without a phase argument still prepare, test, and report in one command.

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

`--affected-plan` must be the first coverage-script argument (or follow
`--prepare` for phased execution). It replaces the
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

For repeated local CLI runs, reuse an isolated generated-project build cache:

```bash
MF_TEST_BUILD_CACHE_DIR="$PWD/target/nextest-build-cache/local" \
  nix develop --command cargo nextest run --locked --all-features -p mf-cli
```

Do not run concurrent nextest processes against the same cache directory. Use a
separate directory for coverage, whose instrumentation has different artifacts.
For repeated local coverage runs, reuse a generated-project build cache:

```bash
nix develop
export MF_TEST_BUILD_CACHE_DIR="$PWD/target/nextest-build-cache/local-coverage"
bash scripts/run-cov.sh
```

The coverage script forwards additional arguments to `cargo nextest run`,
so explicit test filters remain available for focused local checks. These
checks supplement the required hooks and affected tests.
