# CI tests

Pull requests select packages from changes since the nearest compatible successful
validation. CI compares the previous tested merge snapshot directly with the current
merge snapshot. Reuse requires the same actual target-base commit, an ancestor PR
head, compatible execution settings, and a completed successful workflow attempt.
Without that evidence, CI selects from the entire PR diff against the current
merge's actual first parent. Stale base metadata from the event is never used.

All analysis, compilation, and execution use the current merged tree. The selection
retains changed packages and every declared reverse dependency. Known repository
configuration is deferred to full main/nightly verification; unknown inputs or
analyzer failures select the workspace. An empty plan skips Rust tests and reports
coverage as not applicable, while lint and build checks still apply.

Selected tests run once with coverage. The local gate measures changed executable
lines over the same comparison and enforces `PR_COVERAGE_TARGET`. The final
`validation` job requires lint, build, and coverage success, verifies the PR title
check, and publishes an attempt-bound success record. Failed, cancelled, incomplete,
or unpublishable validation cannot advance the reusable baseline.

Pushes to `main` and manual dispatch without PR inputs run complete Linux coverage.
Nightly retains complete Linux/macOS verification. Test or report failures fail CI.

Formatting, package builds, Clippy, repository hooks, and CI script tests
also run in CI. Main additionally builds and checks the macOS package;
complete macOS test execution runs in nightly.

Coverage dependencies and generated-project builds use persistent caches.
The script removes previous workspace binaries and report data while retaining
dependencies, so stale coverage mappings cannot contribute to a new report.
Reporting uses a temporary hard-linked binary view that omits `.kache-prestage-*`
compiler-cache backups. These backups can survive Cargo cleanup and contain old
source mappings; source files and coverage thresholds are not excluded or changed.
The temporary view is removed after report generation without modifying the cache.
Preparation records pre-existing untracked profile paths. Package execution and
reporting collect only new default LLVM profiles left by credential-isolated
subprocesses into the current session's data directory. Pre-existing profiles are
never merged, and generated outputs do not leave a clean tested checkout dirty.
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

Main, nightly, and manual full-suite runs upload LCOV with the `rust` flag for
project coverage. Automatic report discovery and carry-forward remain disabled.
PR runs retain their measured LCOV and incremental gate result as Actions artifacts;
they do not upload a partial increment as cumulative PR patch coverage. The old
Codecov patch status is disabled and the required local `coverage`/`validation`
checks provide the incremental coverage decision.

The `coverage` artifact contains current LCOV and source provenance. The
`pr-validation-input-<run>-<attempt>` artifact contains context, comparison, selected
plan, and gate result. Only the final successful aggregate job publishes
`pr-validation-<run>-<attempt>` for future reuse. Failed gates retain report artifacts
when report generation succeeded, but never publish a successful validation record.

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

To reproduce selected test execution, pass the saved CI plan to the coverage
script. The following local example selects cumulative PR scope; it does not
resolve a successful CI baseline or reproduce an incremental gate:

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

Input classification follows this order in both the CI selector and the opt-in
Flow planner:

| Input | PR Rust selection |
| --- | --- |
| Files inside a workspace package, including manifests, fixture lockfiles, documentation, and deleted sources | Owner and transitive reverse dependencies |
| Root `Cargo.toml` / `Cargo.lock`, Flake/toolchain, Cargo and nextest configuration | No additional packages; full main/nightly runs provide the backstop |
| `docs/`, `openspec/`, `.github/`, known root documentation, lint/release metadata, and CI-only scripts | No additional packages; run relevant non-Rust checks separately |
| `examples/`, `scripts/nextest-build-cache.py`, `scripts/nextest-cargo.sh`, `scripts/cargo_cache.py` | `mf-cli`, `mf-compiler`, and reverse dependencies |
| `scripts/check-release-support.sh` | `mf-cli` and reverse dependencies |
| `scripts/affected_tests.py`, `tools/test-affected/flows/`, `tools/test-affected/run-tests.py` | `mfn-test-affected` and reverse dependencies |
| Other inputs, including removed package sources, or missing all expected shared consumers | Full workspace |

The bounded deferred-file list lives in each selector's `deferred_input` helper;
plans expose matched paths in `ignored_files`. Package ownership takes priority,
so a package fixture named `Cargo.toml` or `README.md` is not ignored. Main and
nightly still execute full suites. Deferring dependency and execution configuration
can delay detection of cross-package regressions until those runs; this is an
explicit PR coverage tradeoff. Unknown paths remain conservative. Update consumer
mappings when shared executable inputs gain new users.

Build scripts and generated runners can consume inputs beyond Cargo's declared
graph, so selected scope is not a proof of semantic independence.

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

### Incremental validation records

`scripts/pr_validation.py` resolves reusable validation from completed successful
CI attempts. A record binds repository/PR, workflow run and attempt, PR head,
actual merge base, tested merge commit/tree, execution signature, comparison,
selected packages, and successful coverage disposition. Artifacts are named
`pr-validation-<run-id>-<attempt>` and contain only `validation.json`.

Eligibility requires the same actual target base and execution signature and an
ancestor PR head. Selection prefers the nearest ancestor, regardless of completion
order. Candidates are ranked by source ancestry before downloading records, and
discovery stops at the nearest usable record. Unrelated or malformed older records
do not invalidate another proven ancestor. Failed/cancelled/pending attempts never
advance the baseline. Discovery is bounded; unavailable history or malformed/expired records resets to cumulative PR
affected scope. This reset does not force workspace-wide tests or change the
existing root dependency/configuration deferrals. Local selectors never fetch;
only CI orchestration may retrieve a recorded merge object.

CI uses this helper for baseline resolution. The final `validation` job checks all
required job results before publishing a record; future discovery also verifies the
producing workflow attempt ultimately completed successfully.

For a validated prior merge snapshot, direct comparison is explicit:

```bash
nix develop --command python3 scripts/affected_tests.py --base "$VALIDATED_MERGE" --comparison snapshot --head HEAD --json
```

This requires a clean checkout of the tested head and local comparison history.
The returned `comparison_mode` distinguishes direct snapshots from ordinary
merge-base selection. Keep `--commit HEAD` for initial cumulative PR selection;
`--base <branch>` retains its merge-base default. A CI resolver chooses a reusable
successful snapshot; the local selector never infers CI success or fetches refs.

### Incremental coverage gate

The local gate uses the exact selection diff and current LCOV executable-line
mappings. Coverage preparation writes `result/source.json` with the source
commit/tree, clean-checkout state, toolchain, target, features, and profile. A
mismatched identity or missing/malformed expected report fails validation.
Duplicate LCOV records are unioned by source line, so repeated compilation cannot
inflate the denominator. New and renamed files participate at their current paths;
deletions still select tests but add no current executable lines.

An absent source mapping is not a coverage exemption. Only independently proven
non-executable input can pass without a mapping: blank/line-comment files, or a
restricted grammar of imports and out-of-line module declarations with known
`cfg`, `path`, or `macro_use` attributes. Executable items, inline modules, unknown
attributes, and macro invocations do not qualify. The allowed built-in attributes
cannot be shadowed by user-defined attributes; see the [Rust name-resolution rules](https://doc.rust-lang.org/reference/names/name-resolution.html). Other unmapped Rust source needs a suitable instrumented build or an
explicit diagnosis; cumulative reruns do not automatically waive this condition.
A confirmed zero denominator is `not_applicable`, not 100% coverage.

The incremental target is configured once as `PR_COVERAGE_TARGET` in CI. The
migration sets 95%, above the verified effective 94.89% Codecov auto target on
`b1135c0`; it does not lower the gate when selection shrinks. Records include the
target in compatibility identity. Main/nightly project coverage keeps its current
policy. The evaluator and final record job enforce the same recorded target.

### Explicit cumulative PR reruns

Dispatch CI on the PR branch with its number and exact current tested merge SHA:

```bash
gh workflow run CI.yaml --ref "$PR_BRANCH" -f pr="$PR_NUMBER" -f merge_sha="$TESTED_MERGE"
```

All jobs check out the supplied merge SHA. The helper verifies its PR head against
the requested PR and workflow run; selection and coverage both use the cumulative
PR diff. A successful run can establish a new baseline. Omit both inputs for the
existing full-suite manual run. An unavailable or mismatched merge is an error.

The main-branch ruleset requires `validation` in addition to coverage, lint, title,
and Ubuntu build checks. The replacement status was demonstrated successfully
before being required. Codecov patch was not a required status in this ruleset.
Re-run all jobs when a failed attempt lacks its own validation-input artifact;
inputs from a different attempt are never substituted.
