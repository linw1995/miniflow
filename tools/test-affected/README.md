# test-affected

Repository developer tooling for [issue #91](https://github.com/linw1995/miniflow/issues/91).
The unpublished `mfn-test-affected` package shares workspace checks but is not
a product release dependency, support package, or bundled example.

One Flow routes local and CI inputs through `builtin.parameters` and `builtin.if_else`.
The local branch builds a comparison request with Code. The CI branch uses Code, Loop,
and bounded GitHub/Git reads to find the nearest compatible successful validation.
Exclusive input edges converge both requests before the shared `git.diff` (Task) ->
`test_affected.plan` (Task) path.
It produces a test plan and optional validation provenance; a separate script consumes the plan.
Ordinary tests use a local fake server. Use `nix develop` for tools.

Invariant checks reuse the upstream `builtin.assert` structured conditions without input or output ports.
Explicit node-success edges order Task predicate producers, assertions, and dependent Tasks.
The search Loop gates its assertion through the `done` output prerequisite because Loop containers
cannot source node-success edges. Condition references alone do not schedule execution.
Standalone interruption uses the upstream runner's cancellation handling, while providers and
the test consumer retain ownership of child-process cleanup.

```sh
nix develop --command cargo check -p mfn-test-affected
nix develop --command python3 -m unittest discover -s scripts -p test_tool_boundaries.py
```

## Git facts and test plans

`github.read` is a Task for bounded read-only GitHub evidence. It accepts an API `path`,
optional scalar `query` object, optional secret `token`, and optional absolute Unix-millisecond
`deadline`. Configuration selects `api_base`, `required`, `timeout_ms`, byte limits, an optional
ZIP `member`, and JSON Pointer `pointer`/`fields` projection. The result contains `available`,
`value` (`data` and archive `members`), a categorical `reason`, and `remaining_ms`, plus the shared
`deadline`. Evidence also has structural limits to fit downstream Code budgets.
Required acquisition errors fail; optional historical errors remain explicit unavailable evidence.
Cancellation always fails. The node neither chooses records nor interprets validation policy.

`github.download` is the file-materializing counterpart for artifacts too large or too
binary for JSON evidence, such as coverage graphs. It downloads one artifact archive and
extracts its configured plain-name `members` into an absolute `directory`, optionally
returning one parsed `metadata_member` (bounded JSON) for in-flow validation. It shares
the `github.read` path, token, deadline, budget, and availability contract.

Both GitHub readers serve the two acquisition channels: PR-run validation records
(`pr-validation-<run>-<attempt>`) and default-branch coverage graphs
(`coverage-graph-<run>-<attempt>`). The coverage-graph branch lists successful
default-branch push runs, orders candidates by ancestry distance from the resolved
comparison base, and downloads the nearest artifact whose metadata binds the run and the
flow's `graph_policy` schema version. The plan node revalidates the graph itself
(commit record, run identity, schema version, and execution signature) before use; every
mismatch degrades to a recorded `coverage_graph` reason, never an error. Local planning
shares the same branch and stays offline unless `github_repository` is supplied.

`git.inspect` reads the current commit/tree/parents, clean/shallow state, and optional
`revisions` relative to `ancestor_of`, reporting availability and ancestor distance.
It does not fetch. `git.fetch` explicitly obtains one full `commit` ID through a configured
remote (default `origin`), leaving caller refs, index, and worktree intact. Both accept
`repository` and optional `deadline`, share the evidence result contract, and bound owned
process execution and captured output. Required inspection failures stop execution;
unavailable historical fetches are data for the Flow to interpret.

`git.diff` accepts `repository`, `base`, optional `head` (default `HEAD`), optional absolute
`artifacts`, and optional `options`. It resolves local revisions and compares their merge base
with head. Snapshots are detached Git worktrees under `target/test-affected/snapshots/` by default;
they remain available after the Flow finishes. Missing refs are errors, not automatic fetches.
Set the Git node's `comparison_mode` configuration to `snapshot` for a direct comparison
between two supplied local snapshots. The default `merge-base` retains branch-comparison behavior.
The plan records the selected mode and resolved revisions. CI baseline eligibility remains
outside the node; the node never fetches missing historical merge objects.

Alternatively, supply `request` with `mode` (`worktree`, `commit`, `merge-base`, or `snapshot`),
optional `base`, `head` (default `HEAD`), `require_checkout`, `options`,
opaque `validation` provenance, and optional opaque
`execution` context forwarded to planning. Do not combine it with legacy `base`/`head`/`options` inputs.
Worktree mode snapshots current files through a temporary copy of the caller's index,
including staged ignored files and nonignored untracked files, without changing the original
index or branch. Commit mode uses the first parent, or an empty snapshot for a root commit.
`require_checkout` enforces a clean caller checkout at the requested head for CI.
Set `options.exact_tests` to false for conservative package-only planning.

`test_affected.plan` consumes the opaque comparison context. It uses
Cargo metadata for all declared dependency kinds and owns the shared
[input policy](../../docs/ci.md#affected-package-tests). Package manifests retain owner/consumer scope; known repository configuration
is deferred to full main/nightly verification. Deleted sources are attributed through the base
workspace; when their owning package is absent from the compared head they are deferred instead of
forcing full-workspace fallback. Known shared executables select their consumers, while unknown
inputs retain full-workspace fallback. Source diffs are the impact seed.

With test precision, syntax-tree comparison first selects only added or modified recognized
test functions when all other source items remain unchanged, without requiring coverage history.
Ordinary inline/external modules in library, binary, and integration-test targets are resolved
to exact identities for `#[test]`, `#[tokio::test]`, and `#[async_std::test]`. Deleted/renamed tests,
helper changes, unsupported attributes or module layouts, and tests called as helpers retain
the existing graph/package fallback. Package precision disables this refinement.

A resolved coverage graph narrows other changed Rust files to the
exact tests covering its base-side line ranges. Files the graph predates (changed between the graph
commit and the comparison base), files with uncovered changed ranges, and files absent from the graph
widen back to the existing package plus reverse-dependency scope; every widening records its reason in
the plan's `coverage_graph` outcome next to the graph's commit/run/schema provenance. Graph-derived
selectors flow through the same exact-filter consumers, so identity
mismatches widen to the recorded baseline. Graph unavailability never forces full-workspace fallback;
unknown inputs still do.

The plan records source/tree/toolchain and build-environment identity, conservative `baseline_packages`,
retained whole `packages`, exact `tests` (`package`, `binary`, `name`), reasons, and deferred
`ignored_files`. Older saved plans without `ignored_files` or the coverage-graph outcome, and plans
carrying the removed analysis fields, remain accepted by the consumer. Counts remain null
without a supplied matching inventory. Options accept `configuration` (`target`, `all_features`,
`features`, `no_default_features`) and an optional `inventory` path. Default test configuration is
all features on the host target. The planning path does not build or execute tests.

The separate script validates identities, builds/lists the baseline once, validates exact cases,
and reuses the build for execution with nextest's default profile and no user configuration file.
Cargo artifacts live in `<snapshot>/target/test-affected`, isolating workspace binaries even when
the caller supplies a shared `CARGO_TARGET_DIR`. Listing and execution use the same directory.
Missing exact names widen to baseline. Empty plans invoke no test command; compile/test failures
preserve a nonzero exit code. The script accepts a plan object or the Flow's `{"plan": ...}` output, including an optional
`validation` field forwarded by the plan node.

```sh
nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json --list-only --inventory-out target/test-inventory.json
nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json
nix develop --command python3 -m unittest discover -s tools/test-affected -p test_runner.py
```

Keep plans and inventories under ignored `target/`. Do not remove referenced snapshots until their plans
are no longer needed. Cleanup uses normal `git worktree remove` on explicitly chosen owned snapshots.
Changing a snapshot or compilation configuration requires a new plan. The coverage script
accepts packages and exact cases against its current checkout. It validates cases against the
instrumented nextest inventory, widening missing identities to the conservative baseline.

## Process execution

The unpublished tooling package also registers `process.run` on Unix. Configure `program`
and optional `args` without shell interpolation. Inputs are optional `cwd` and `stdin_json`;
the `result` output is text by default, or one decoded JSON value with `output: "json"`.
JSON input sensitivity propagates to the result. Stderr is a separate diagnostic stream.

The default timeout is one hour (`timeout_ms`); stdin is limited to 1 MiB and stdout to
1 MiB by default (`max_stdout_bytes`, up to 16 MiB). Invalid output, nonzero exit, limits,
and cancellation fail the Task. Cancellation permits up to one second of cooperative
termination before forced group cleanup. Remaining same-group descendants are also
terminated after normal completion. Programs such as the test consumer retain ownership
of nested process groups they create.

## Compile and run the Flow

The definition uses version `2026-10-19`: node-local `inputs` bind data, including
exclusive alternatives; `edges` express branch-output gates and Task-success prerequisites.
Compile once and run the same executable locally or in CI. In this checkout, use the
CLI's development support feature so generated projects use local support crates.
An installed `mf` with the appropriate support packages uses the same compile/run commands.

```sh
nix develop --command cargo run -p mf-cli --features development-support -- \
  compile tools/test-affected/flows/test-affected.json \
  --output target/test-affected-runner --build-dir target/test-affected-build
nix develop --command cargo run -p mf-cli --features development-support -- \
  run ./target/test-affected-runner --params.repository "$PWD" \
  --params.mode worktree --params.run true
```

Omit `--params.run true` to emit only a plan. Enabled local execution invokes the existing
consumer through `process.run`, passing the plan on stdin. The consumer also accepts a path
or `-` directly; its stdout contains one JSON report and child diagnostics use stderr.
The execution branch uses the consumer at `tools/test-affected/run-tests.py` in the compared
repository. Custom definitions can configure another program or consumer path.

The local compile command retains telemetry support so TUI can observe execution.
To inspect execution, add `--tui` to `mf run`. The default Execution view shows execution
prerequisites and bundled data dependencies, hiding direct connections already represented
by another dependency path. This changes only the display; input bindings and execution
conditions remain intact.
Press `e` to cycle through Dependencies (all bundled data mappings) and Ports (individual
mappings); Details retains the input sources. Select `search` and
press `l` to inspect its Loop body, then `h` to return. Rebuild both the CLI and runner
when upgrading from an older manifest format.

Local parameters:

| Parameter | Default | Meaning |
| --- | --- | --- |
| `ci` | `false` | Select CI explicitly with `--params.ci true`; environment variables do not switch branches |
| `run` | `false` | Execute the local plan through the existing consumer; rejected with `ci=true` |
| `repository` | `.` | Repository to analyze |
| `mode` | `worktree` | `worktree`, `commit`, `merge-base`, or `snapshot` |
| `base` | null | Local base ref for merge-base or snapshot comparison |
| `head` | `HEAD` | Commit or head ref to analyze |
| `precision` | `tests` | `tests` permits source and coverage-graph narrowing; `packages` keeps package scope |
| `configuration` | `{}` | Local target/features/all-features/no-default-features settings |
| `inventory` | null | Optional matching nextest inventory |
| `artifacts` | null | Optional absolute snapshot directory |

For one commit, set `--params.mode commit --params.head HEAD`. For branch comparison,
set `--params.mode merge-base --params.base main --params.head HEAD`. Snapshot mode compares
supplied refs directly. Worktree snapshots preserve caller files and index, and remain available
for the consumer. Relative dependency paths resolve against the definition, not the invocation directory.
The `--inputs` and `--inputs-file` interfaces also accept these qualified parameter names.

Enable the CI branch with `--params.ci true`. It binds repository/run/attempt and event
parameters from GitHub environment variables.
The caller supplies `MF_PR_NUMBER`, `MF_PR_BRANCH`, `MF_PR_HEAD`, `MF_TOOLCHAIN` (`rustc -vV`),
and optionally `MF_COVERAGE_TARGET` (95 by default). `GH_TOKEN` is a secret parameter supplied
only to GitHub reads. Manual PR dispatch supplies `MF_MERGE_SHA` and must identify the current
pinned merge. `MF_CUMULATIVE=true` bypasses historical candidate discovery. Local execution
needs none of these CI values and performs no GitHub request.

CI enables exact-test refinement under the `flow-pr-v3` compatibility policy.
Production changes not resolved by a compatible coverage graph retain conservative package scope. Candidate
metadata is inspected first; artifacts and missing merge objects are read lazily in source
ancestry order. A usable record stops the Loop immediately. Incompatible or unavailable
history produces a cumulative affected comparison, while invalid current identity fails.

Plan-only output is one `{"plan": ..., "validation": ...}` record. Local validation is null; CI
validation contains the current execution context and comparison for coverage and final recording.
Enabled local execution adds an `execution` report with the selected count, execution status,
and inventory/test durations. Empty plans return `executed: false`; failed tests fail the Flow.
CI retains external coverage and success-record publication. Source compilation/build-cache
preparation, Flow evidence/analysis/planning, and actual test execution remain separate costs.

CI invokes this same Flow and consumes mixed package/exact-test plans. Coverage preparation checks
the runnable inventory before executing each package's filter. Complete crate suites may upload
source-scoped coverage; partial suites retain reports only as artifacts and for the incremental gate.
Measure compilation, evidence discovery, planning, and script build/list/test time separately.
The policy identifier and validation record version live in the `validation_policy` constant.
