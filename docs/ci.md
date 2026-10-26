# CI tests

Pull requests select tests from changes since the nearest compatible successful
validation. CI compares the previous tested merge snapshot directly with the current
merge snapshot. Reuse requires the same actual target-base commit, an ancestor PR
head, compatible execution settings, and a completed successful workflow attempt.
Without that evidence, CI selects from the entire PR diff against the current
merge's actual first parent. Stale base metadata from the event is never used.

All analysis, compilation, and execution use the current merged tree. The selection
starts with changed packages and every declared reverse dependency. Production changes
retain package scope unless a compatible coverage graph narrows them
to exact test selectors. Known repository
configuration is deferred to full main/nightly verification; unknown inputs select the
workspace. An empty plan skips Rust tests and reports
coverage as not applicable, while lint and build checks still apply.

Selected tests run once with coverage. The local gate measures changed executable
lines over the same comparison and enforces `PR_COVERAGE_TARGET`. The final
`validation` job requires lint, build, and coverage success, verifies the PR title
check, and publishes an attempt-bound success record. Failed, cancelled, incomplete,
or unpublishable validation cannot advance the reusable baseline.

Pushes to `main` and manual dispatch without PR inputs run complete Linux coverage.
Nightly retains complete Linux/macOS verification. Test or report failures fail CI.

A successful default-branch push coverage run also collects per-test line coverage
(a nextest run-wrapper gives each test its own LLVM profile) and indexes it into an
immutable, versioned SQLite coverage graph mapping workspace line intervals to the
distinct covering tests. Each graph binds its commit, producing run and attempt,
schema version, and execution signature (platform-independent toolchain identity —
the `rustc -vV` release and commit hash, without the host triple — plus features,
target, profile, and coverage-relevant environment). Since line-to-test attribution
does not depend on the host architecture, a Linux-produced graph is consumable
elsewhere: foreign platform selectors widen to the recorded baseline through
inventory identity matching, and foreign covered lines can only over-select. Publication rides the existing
artifact pipeline as `coverage-graph-<run>-<attempt>` (`graph.sqlite` plus
`graph.metadata.json`) and happens only when the full default-branch coverage
completes successfully; PR, dispatch, and local runs never collect or publish a
graph. A full-suite graph measures about 30 MB per commit; every main run
regenerates it, only the latest compatible ancestor matters to consumers, and the
standard Actions artifact retention applies. A local full-coverage run opts into
the same collection with `scripts/run-cov.sh --coverage-graph <path>`, which
cannot combine with `--affected-plan` and never publishes.

Graph exports retain workspace line records only; function and branch records are
not consumed by the index. Each LLVM subprocess uses one worker while tests export
in parallel, avoiding nested thread pools. Export, indexer-build, and indexing times
are reported separately. Aggregate coverage reports retain their existing coverage data.

Formatting, package builds, Clippy, repository hooks, and CI script tests
also run in CI. Main additionally builds and checks the macOS package;
complete macOS test execution runs in nightly.
Nix prepares separate dependency artifacts for the release CLI build, workspace
Clippy checks, and full nextest suite. Each preparation runs only the Cargo phase
its consumer needs, with matching package, target, feature, and profile settings.
Lint and coverage share a Nix development-environment cache keyed by the toolchain,
flake, and setup action, independent of repository sources. Build checks retain
source-keyed stores. Nightly first restores its test store, then falls back to the
same platform's check store to reuse package and Clippy artifacts on a cold run.

Coverage dependencies and generated-project builds use persistent caches.
Coverage preparation defaults dev/test debug information to zero while retaining
LLVM coverage maps and honoring caller overrides. These internal defaults are
applied after recording caller inputs, just like coverage instrumentation, so
default CI and local graph signatures remain compatible.
Instrumented dependency caches are keyed by toolchain, manifests, coverage settings,
and compiled package scope; exact hits avoid uploading the same dependencies again.
Generated-project dependency and vendor seeds under `target/coverage-fixtures` use
stable toolchain, manifest, and cache-layout keys. Packaged-project builds use a
separate source-keyed cache, so source changes do not re-upload the dependency seed.
Both remain outside the instrumented target's identity cleanup.
Older caches within the same layout version and runner platform remain dependency
seeds. Compiler/flag identities, Cargo fingerprints, workspace cleanup, and packaged
source digests still validate their contents before reuse.
The script removes previous workspace binaries and report data while retaining
dependencies, so stale coverage mappings cannot contribute to a new report.
Reporting uses a temporary hard-linked binary view that omits `.kache-prestage-*`
compiler-cache backups. These backups can survive Cargo cleanup and contain old
source mappings; source files and coverage thresholds are not excluded or changed.
The view includes current dependency artifacts and build-script binaries. Fixtures
that delete their build directories retain executed binaries in the session-scoped
`MF_COVERAGE_BINARIES_DIR`. For copied sources, a `*.paths.json` file maps absolute
build-time source filenames to checkout-relative filenames for grcov. Conflicting
mappings fail reporting instead of silently selecting one source.
Preparation clears this directory so previous fixture mappings cannot leak into a
new run. The temporary view is removed after reporting without modifying the cache.
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
running the relocated host. Ordinary generated builds use one serialized test group.
The LTO/strip probe has its own optimized target, vendor tree, setup session, and
serialized group, so it can overlap ordinary fixture tests without replacing their
runners. Its setup prepares sources only; optimized compilation runs inside the test
instead of serially delaying the entire suite. Packaging remains serialized with
ordinary builds while seeding its separate target. Packaging and LTO each reserve
two nextest slots instead of blocking all tests.
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

PR validation generates LCOV and selection/source metadata only. Main, nightly,
and local runs retain HTML, Cobertura, LCOV, and Markdown reports by default.
Set `MF_COVERAGE_REPORT_FORMAT=lcov` for compact local reports. Reporting logs
separate timings for profile collection, binary preparation, grcov, publication,
and removal of the temporary binary view.

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
profiles and workspace binaries, and builds the selected suites. Exact-case plans
build their conservative baseline once so missing runnable identities can widen
selection without rebuilding. The actual nextest inventory resolves per-package
filters and promotes exact selections covering a complete suite to package scope.
Package steps accumulate profiles, and reporting merges them. Local invocations
without a phase argument still prepare, test, and report in one command.

## Codecov reports

Main, nightly, and manual full-suite runs upload LCOV with the `rust` flag for
project coverage. Automatic report discovery and carry-forward remain disabled.
Each successfully completed crate suite also produces a source-scoped
`crates/<package>.lcov` report, uploaded in its own step with a package-named flag.
PRs upload only these complete crate reports. A package with only some tests selected
retains partial LCOV in Actions artifacts and still participates in the changed-line
gate; its partial report is never uploaded as complete crate coverage. Reports omit
dependency-owned sources and crates without source mappings. Completeness refers to
the configured nextest suite.
Main/nightly publish the same crate flags alongside the full `rust` report, without
carry-forward. Crate reports are not cumulative PR patch coverage. The old
Codecov patch status is disabled and the required local `coverage`/`validation`
checks provide the incremental coverage decision.

The `coverage` artifact contains current LCOV, source provenance, and `selection.json`
with complete/partial suites, inventory fallback, execution completion, and published
crate reports. The
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

One planning Flow serves local and PR CI execution. `builtin.if_else` selects the local
comparison path or the CI success-history path; exclusive input bindings converge before
Git comparison and the shared Rust planner. Cargo metadata supplies
package ownership and every declared local dependency kind, including inactive optional,
platform, test, and build dependencies. CI narrows through a compatible coverage graph;
local callers may request the same with test precision. Main and nightly retain full suites.

From the repository root, compile once with pinned tools:

```bash
nix develop --command cargo run -p mf-cli --features development-support -- compile \
  tools/test-affected/flows/test-affected.json --output target/test-affected-runner \
  --build-dir target/test-affected-build

# Snapshot staged, unstaged, and nonignored untracked files without changing the index.
nix develop --command cargo run -p mf-cli --features development-support -- run \
  ./target/test-affected-runner --params.mode worktree \
  --params.precision tests > target/test-plan.json

# Execute the snapshot-bound selection, preserving empty and failed plans.
nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json
```

For planning and execution in one command, pass `--params.run true` to the same Flow:

```bash
nix develop --command cargo run -p mf-cli --features development-support -- run \
  ./target/test-affected-runner --params.precision tests --params.run true
```

Execution adds an `execution` report to the output. Test diagnostics use stderr; failure
makes the Flow fail. The default remains planning-only. CI rejects `run=true` and continues
to use the coverage pipeline rather than this local nextest consumer.

Use `--params.mode commit --params.head HEAD` for a first-parent comparison, including
root commits. Use `--params.mode merge-base --params.base origin/main --params.head HEAD`
for a branch comparison, or `snapshot` to compare supplied local revisions directly.
Local modes do not fetch refs and query GitHub only for coverage graph acquisition when
a repository identity is supplied. Worktree mode creates an immutable runnable
snapshot; committed comparisons do not include uncommitted edits. CI requires a clean
checkout of the actual tested merge.

The output contains `plan` and optional CI `validation` provenance. Plans report whole
`packages`, exact `tests`, conservative `baseline_packages`, reasons, deferred
`ignored_files`, and source/configuration identity. `packages` precision keeps package
scope. `tests` precision narrows test-only source changes directly and may narrow other
changes through a compatible coverage graph. The consumer handles
both scopes and never turns an empty plan into unfiltered nextest execution.

With `tests` precision (the default for local planning and enabled in PR CI), syntax-tree
comparison first selects only added or modified test functions when all other source items
remain unchanged. This works offline without a coverage graph. It resolves ordinary inline
and external modules in library, binary, and integration-test targets, and recognizes
`#[test]`, `#[tokio::test]`, and `#[async_std::test]`. Deleted or renamed tests, helper changes,
unrecognized attributes, ambiguous module layouts (including `#[path]` and `include!`),
and tests referenced as helpers retain the existing graph/package fallback. Exact identities
are checked against the runnable head inventory before execution; mismatches restore the
conservative baseline. Package precision disables both source and coverage refinement.

A compatible coverage graph can also narrow production changes
below package scope. Planning resolves the nearest compatible default-branch ancestor
graph of the comparison base through the same lazy, bounded, ancestry-ordered artifact
acquisition used for validation records, then maps each changed base-side line range to
the exact tests that covered it. Discovery inspects up to the latest 100 successful
main runs; a full page remains usable, and only candidates within that window are
considered. Missing compatible evidence still widens conservatively. The CI summary
includes graph provenance or the acquisition failure reason.
Files that changed between the graph commit and the
comparison base, changed ranges with no covering test, and files absent from the graph
widen back to package plus reverse-dependency scope with per-file reasons. The plan
records graph provenance (commit, run, attempt, schema version) and every widening under
`coverage_graph`. An unavailable, incompatible, or malformed graph degrades to
package-level selection with a recorded reason and never forces full-workspace fallback;
unknown inputs still do. Graph-derived selectors flow through the
same exact-filter consumers, so identity mismatches against
the runnable inventory widen to the recorded baseline. Local planning acquires graphs only
when `--params.github_repository` is supplied (authenticated through `GH_TOKEN`), and
stays offline otherwise; source-based test refinement remains available.

The coverage script accepts whole packages and exact test identities and executes its
current checkout; it does not implement the snapshot consumer. To reproduce a saved
CI selection, use its `plan.json` with the matching clean tested checkout. For a local
committed comparison, generate a package-only plan and extract it:

```bash
nix develop --command cargo run -p mf-cli --features development-support -- run \
  ./target/test-affected-runner --params.mode merge-base \
  --params.base origin/main --params.head HEAD --params.precision packages > target/test-plan.json
jq '.plan' target/test-plan.json > target/affected-packages.json
nix develop --command bash scripts/run-cov.sh --affected-plan target/affected-packages.json
```

`--affected-plan` must be the first coverage-script argument (or follow `--prepare`).
It replaces workspace selection with explicit packages and exact-case filters. Empty selections
clear stale reports and do not invoke Cargo. Malformed selections fail; missing runnable exact
identities or oversized filter expressions widen to the recorded conservative baseline.
Other nextest arguments can follow the path. For mutable local source, use the snapshot-aware
test consumer or run local coverage explicitly after choosing its package scope.

Input classification has one owner in the shared Flow planner:

| Input | PR Rust selection |
| --- | --- |
| Only added or modified recognized test functions, with source/module identity proved | Exact changed cases, without coverage history |
| Files inside a workspace package, including manifests, fixture lockfiles, documentation, and deleted sources | Owner and transitive reverse dependencies |
| Deleted sources whose base-side package is absent from the compared head | No additional packages; full main/nightly runs provide the backstop |
| Root `Cargo.toml` / `Cargo.lock`, Flake/toolchain, Cargo and nextest configuration | No additional packages; full main/nightly runs provide the backstop |
| `docs/`, `openspec/`, `.github/`, known root documentation, lint/release metadata, and CI-only scripts | No additional packages; run relevant non-Rust checks separately |
| `examples/`, `scripts/nextest-build-cache.py`, `scripts/nextest-cargo.sh`, `scripts/cargo_cache.py` | `mf-cli`, `mf-compiler`, and reverse dependencies |
| `scripts/check-release-support.sh` | `mf-cli` and reverse dependencies |
| `tools/test-affected/flows/`, `tools/test-affected/run-tests.py`, and removal of the legacy selector | `mfn-test-affected` and reverse dependencies |
| Other unclassified inputs, or known shared inputs missing all expected consumers | Full workspace |

The bounded deferred-file list lives in the Rust planner's `deferred_input` helper;
plans expose matched paths in `ignored_files`. Deletions resolve ownership from the
comparison base before the head, so a package that still exists keeps its deleted
sources while a removed package defers instead of becoming an unknown input. Package
ownership takes priority, so a package fixture named `Cargo.toml` or `README.md` is
not ignored. Main and
nightly still execute full suites. Deferring dependency and execution configuration
can delay detection of cross-package regressions until those runs; this is an
explicit PR coverage tradeoff. Unknown paths remain conservative. Update consumer
mappings when shared executable inputs gain new users.

Build scripts and generated runners can consume inputs beyond Cargo's declared
graph, so selected scope is not a proof of semantic independence.

Test precision is the default; test-only changes narrow from source, and other Rust changes
narrow through the coverage graph when available. Unproved changes retain conservative
package scope. Invalid
Git refs, missing required local history, mismatched snapshots, and failed Cargo metadata
stop execution rather than produce an unreliable plan. Declared dependency closure is
conservative, not a proof of arbitrary program effects or cross-platform independence.

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

The CI branch resolves reusable validation using existing Code, Loop, and conditional
nodes, backed by bounded GitHub/Git evidence providers. A record binds repository/PR, workflow run and attempt, PR head,
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

CI invokes the compiled Flow through `mf`. Its native planning executables are cached by
source, manifest, definition, and pinned-toolchain identity; cache misses rebuild them and
cannot alter selected scope. A separate cache retains Cargo dependencies for both the CLI
and generated planning runner across source revisions. It prefers matching manifests and
lockfiles, then falls back within the same pinned toolchain and build configuration.
Its key excludes the commit SHA because saved checkout artifacts are pruned; exact
dependency hits avoid re-uploading the same seed after source-only changes.
The generated project's ownership marker travels with its target directory; CLI version
and build-layout inputs partition every dependency-cache restore prefix.
Restored checkout artifacts are removed before building; workspace artifacts and generated
executables are pruned again before saving. Final executable reuse still requires an exact
source identity, and an executable-cache hit skips dependency-cache transfer entirely.
Main pushes warm both caches in the lint job alongside coverage, using the same composite
action as PR coverage. This publishes a default-branch cache that new PRs can restore;
PR-scoped cache entries otherwise benefit only subsequent runs of that PR.
`pr_validation.py accept` only binds the returned plan to the
current execution and writes handoff artifacts. The `flow-pr-v2` policy resets incompatible
pre-migration records to cumulative scope. The policy identifier and record version live
in the Flow's `validation_policy` constant and are read by the publication guard.

The final `validation` job checks all
required job results before publishing a record; future discovery also verifies the
producing workflow attempt ultimately completed successfully.

For a validated prior merge snapshot, direct comparison is explicit:

```bash
nix develop --command cargo run -p mf-cli --features development-support -- run \
  ./target/test-affected-runner --params.mode snapshot \
  --params.base "$VALIDATED_MERGE" --params.head HEAD --params.precision packages
```

Local history must already be available. The returned `comparison_mode` records how the
range was resolved; the local branch never infers CI success. Only the explicitly selected
CI branch may retrieve missing recorded merge objects. Initial/cumulative PR comparison
uses the tested merge's actual first parent.

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
`cfg`, `path`, or `macro_use` attributes. In mixed modules, the evaluator can also
prove that only those declarations changed by comparing all remaining top-level
items verbatim against the comparison base. It uses the pinned `rust-analyzer` parser to locate item boundaries and rejects malformed syntax.
Changed executable items, inline modules, unknown attributes, and macro invocations
do not qualify. The allowed built-in attributes
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

All jobs check out the supplied merge SHA. The Flow verifies its PR head against
the requested PR and workflow run; selection and coverage both use the cumulative
PR diff. A successful run can establish a new baseline. Omit both inputs for the
existing full-suite manual run. An unavailable or mismatched merge is an error.

The main-branch ruleset requires `validation` in addition to coverage, lint, title,
and Ubuntu build checks. The replacement status was demonstrated successfully
before being required. Codecov patch was not a required status in this ruleset.
Re-run all jobs when a failed attempt lacks its own validation-input artifact;
inputs from a different attempt are never substituted.
