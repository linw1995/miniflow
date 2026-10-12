# CI tests

Pull requests run `scripts/run-cov.sh --refresh-affected-cov`. They restore
an isolated baseline for the same PR and build fingerprint first, then fall
back to main mappings. Successful collection writes a new snapshot keyed by
PR, host, fingerprint, and tested merge commit. Failed or cancelled runs do
not publish snapshots. Main never restores PR namespaces.

The PR baseline cache contains `coverage.db`, `baseline.sha`, `commits.bundle`,
and compact per-test profiles under `target/affected/profiles/`. Compilation
artifacts use the independent build cache. The bundle preserves actual
collection coordinates when earlier PR merge refs disappear. GitHub handles
retention through its inactivity and least-recently-accessed eviction rules;
all caches still share the repository's storage limit.

Selected tests execute once. A small patch to pinned cargo-affected 0.4.1
includes explicit input rules in diff collection and exports sparse merged
counters before raw profiles are deleted. Updated tests replace their
profiles; profiles without corresponding database mappings are pruned. Full
fallback clears older profiles. PR reports merge the retained valid per-test
counters against current binaries, preserving earlier commits' patch coverage.
The pinned tool is provided by `nix develop`. Formatting, package builds,
Clippy, and repository hooks still run in CI.

Pushes to `main` run `scripts/run-cov.sh --refresh-affected` instead. Compatible
baselines use `cargo affected collect --diff` to execute and refresh only affected
tests, retaining unaffected rows and their original commit anchors. Missing or
incompatible baselines and untracked build inputs require a full collection.
Input rules can conservatively reselect tests while older anchors remain;
nightly full collection refreshes all mappings and removes that accumulated diff.
This executes the selected tests once; it does not repeat an aggregate coverage
run. The `collection` field in `affected-tests` records the actual collection
mode and reason; any selection fields describe its status preflight.

Successful Linux collections on the default branch publish the coverage
database and its Git commit anchor. PRs use that baseline as a fallback and publish only their own isolated snapshots. The cache fingerprint includes the root manifest and manifests
matching workspace members (`crates/mf-*/Cargo.toml` and
`crates/builtin-nodes/*/Cargo.toml`). Fixture, generated-project, and vendored
manifests cannot change the key between restore and save. The checkout includes full Git history.
Missing baselines, missing Git anchors, and changed build fingerprints fall
back to all tests. Each run uploads `affected-tests`, a JSON selection report.
Collection uses `scripts/run-cov.sh --collect-affected` so its ambient
instrumentation flags match affected runs and do not invalidate the fingerprint.

Coverage-based selection is approximate. Exact-path metadata rules in
`Cargo.toml` map known fixtures and examples to their consuming test binaries:

- Compiler helper fixtures select their specific integration binaries.
- The shared multi-nodes fixture selects CLI and compiler integration tests.
- Telemetry fixtures select contract tests and, where needed, telemetry unit tests.
- Workflow examples select the CLI examples suite and their other consumers.

The wrapper reads those same exact paths before deciding on a full-suite fallback;
it does not maintain a separate input allowlist or reimplement glob matching.
New, renamed, or unmapped fixtures and examples, including Rust files under
`examples/`, and other unmapped non-Rust inputs still require the full suite.
Global test/build configuration (including
`.config/nextest.toml`), build scripts, and the runtime derive crate retain full
execution even if an input rule matches. Update rules when adding consumers.
Known documentation files (`AGENTS.md`, `README.md`, Markdown under `docs/`
and `openspec/`, and OpenSpec `.openspec.yaml` files) are exempt; fixture and
unknown Markdown files without exact rules still force the full suite. Metadata rules in
`Cargo.toml` select CLI and generated-runner integration tests when their Rust
inputs change, because coverage from the test binary cannot observe every
subprocess. Keep these rules current when adding subprocess test suites or
proc-macro crates. This intentionally limits speedups for compiler and runtime
changes. Changes to the root manifest or cache fingerprint initially require
a new full baseline before selective execution becomes available again.

## Codecov reports

PR reports use the `rust-affected` flag and only report patch coverage in
checks and comments. Full reports use both `rust` and `rust-affected`: `rust`
provides project coverage, and `rust-affected` seeds the baseline for patch
comparisons. A full-suite fallback in PR CI can therefore also report project
coverage. When only an incremental report is uploaded, the project status
passes without evaluating full-project coverage, preserving its status context;
the patch status evaluates the uploaded coverage. With no new tests selected, retained PR profiles can still supply the cumulative
patch report; without retained profiles, no report is generated.
The `main` collection job does not generate aggregate coverage reports.
Nightly supplies complete default-branch reports, keeping coverage history
and the badge based on full-suite coverage.

Neither flag uses carryforward. Codecov carries forward a whole missing flag,
not unexecuted tests within a newly uploaded flag. A partial report cannot
replace `rust`, and merging old LCOV hits into a new report could mask genuine
coverage losses. PR patch coverage uses the retained per-test snapshot, replacing rerun tests
rather than merging obsolete aggregate LCOV hits. It does not establish freshly
executed full-suite coverage of unchanged code. See Codecov's
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

For faster development iterations, collect a local baseline from a clean
commit with `scripts/run-cov.sh --collect-affected`, then use `--affected`
after editing Rust sources. Export a persistent `MF_TEST_BUILD_CACHE_DIR`
for both commands to reuse generated-project dependencies. After committing
changes, `--refresh-affected` updates only affected mappings when compatible.
Use the same pinned toolchain and host platform throughout; a Linux CI
database does not match a local macOS fingerprint. Collection requires a
clean working tree so stored source ranges remain aligned with their commits.
