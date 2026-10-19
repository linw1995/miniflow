# test-affected

Repository developer tooling for [issue #91](https://github.com/linw1995/miniflow/issues/91).
The unpublished `mfn-lsp` and `mfn-test-affected` packages share workspace checks but are not
product release dependencies, support packages, or bundled examples.

One Flow routes local and CI inputs through `builtin.parameters` and `builtin.if_else`.
The local branch builds a comparison request with Code. The CI branch uses Code, Loop,
and bounded GitHub/Git reads to find the nearest compatible successful validation.
Exclusive input edges converge both requests before the shared `git.diff` (Task) ->
`lsp.analyze` (Stream) -> `test_affected.plan` (Task) path.
It produces a test plan and optional validation provenance; a separate script consumes the plan. LSP servers are explicitly configured,
never downloaded automatically. Ordinary tests use a local fake server. Use `nix develop` for tools
and the pinned rust-analyzer when running real-server checks.

```sh
nix develop --command cargo check -p mfn-lsp -p mfn-test-affected
nix develop --command python3 -m unittest discover -s scripts -p test_tool_boundaries.py
```

## LSP node

`lsp.analyze` is a `TypedStreamNode`. Its input ports are `request` and opaque `context`;
its outputs are `outcome` and the unchanged `context` (including secret metadata).
A request identifies snapshots by `id`, absolute `root`, and `documents` with relative paths
and UTF-8 byte ranges. `initialization_options` is forwarded to the server unchanged.

The node configuration selects `command`, optional `args`, optional source `extensions`,
and `profile` (`standard` by default, or `rust_analyzer` for its explicit readiness notification). Limits default to 120000 ms, 2048 requests per snapshot, and 16 MiB per result budget.
Queries use statically advertised standard symbol, reference, and call-hierarchy capabilities.
Standard semantic messages use `lsp-types` for parameters, results, and capability declarations.
The bounded transport retains pre-allocation message limits, cancellation, and process ownership;
malformed semantic results become analysis gaps. Protocol-library types remain inside the provider,
while Flow evidence keeps its existing JSON format and opaque call-hierarchy data.
The Rust profile adds readiness handling only; it does not query `relatedTests` or expose an unused
extension-result registry. Dynamic capability registration is not advertised or supported.
The deadline spans the entire batch. Each snapshot gets one session, reused for all its queries.

Every completed batch emits exactly one `analyzed`, `incomplete`, or `not_requested` outcome.
Unsupported capabilities and remote analysis failures are evidence gaps, not proof that tests are
independent. Empty batches do not start a server. Cancellation, invalid requests, cleanup failures,
and emitter failures stop the node without publishing fallback. All child processes are reaped
before the terminal result is admitted. Progress adds diagnostic span events, never business messages.
The initial transport uses Unix nonblocking pipes; other platforms report an unsupported transport.

```sh
nix develop --command cargo nextest run -p mfn-lsp
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
The node's configured `initialization_options` are forwarded to analysis without Rust-specific policy.
Set the Git node's `comparison_mode` configuration to `snapshot` for a direct comparison
between two supplied local snapshots. The default `merge-base` retains branch-comparison behavior.
The plan records the selected mode and resolved revisions. CI baseline eligibility remains
outside the node; the node never fetches missing historical merge objects.

Alternatively, supply `request` with `mode` (`worktree`, `commit`, `merge-base`, or `snapshot`),
optional `base`, `head` (default `HEAD`), `analyze` (default true), `require_checkout`, `options`,
optional opaque `initialization_options`, and opaque `validation` provenance. Do not combine it with legacy `base`/`head`/`options` inputs.
Worktree mode snapshots current files through a temporary copy of the caller's index,
including staged ignored files and nonignored untracked files, without changing the original
index or branch. Commit mode uses the first parent, or an empty snapshot for a root commit.
`require_checkout` enforces a clean caller checkout at the requested head for CI.
Setting `analyze` to false retains Git changes while emitting an empty semantic batch.
Set `options.exact_tests` to false for conservative package-only planning.

`test_affected.plan` consumes only the analysis output and its opaque comparison context. It uses
Cargo metadata for all declared dependency kinds and owns the shared
[input policy](../../docs/ci.md#affected-package-tests). Package manifests retain owner/consumer scope; known repository configuration
is deferred to full main/nightly verification. Known shared executables select their consumers, while
unknown inputs and analyzer failures retain full fallback. Source diffs remain the impact seed even when
call edges do not change. LSP evidence alone never excludes unobserved tests.

The initial narrowing rule accepts directly declared private `#[test]` functions in conventional
library modules or integration targets when only their bodies changed. It checks unchanged surrounding
code, unique test identifiers, known test macros, and absence of out-of-line children or visible shared
resource access in the test scope. Unknown patterns retain package scope. This is a deliberately limited
source ownership rule, not a proof of arbitrary program effects or test independence.

The plan records source/tree/toolchain and build-environment identity, conservative `baseline_packages`,
retained whole `packages`, exact `tests` (`package`, `binary`, `name`), reasons, and deferred
`ignored_files`. Older saved plans without `ignored_files` remain accepted by the consumer. Counts remain null
without a supplied matching inventory. Options accept `configuration` (`target`, `all_features`,
`features`, `no_default_features`) and an optional `inventory` path. Default test configuration is
all features on the host target. No tests or test builds run inside the Flow.

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
Changing a snapshot or compilation configuration requires a new plan. The existing coverage script
accepts package-only selection and must not be used to silently discard exact-case filters.

## Compile and run the Flow

Compile once and run the same executable locally or in CI. In this checkout, use the
CLI's development support feature so generated projects use local support crates.
An installed `mf` with the appropriate support packages uses the same compile/run commands.

```sh
nix develop --command cargo run -p mf-cli --features development-support -- \
  compile tools/test-affected/flows/test-affected.json \
  --output target/test-affected-runner --build-dir target/test-affected-build --no-telemetry
nix develop --command cargo run -p mf-cli --features development-support -- \
  run ./target/test-affected-runner --params.repository "$PWD" \
  --params.mode worktree > target/test-plan.json
nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json
```

Local parameters:

| Parameter | Default | Meaning |
| --- | --- | --- |
| `ci` | `false` | Select CI explicitly with `--params.ci true`; environment variables do not switch branches |
| `repository` | `.` | Repository to analyze |
| `mode` | `worktree` | `worktree`, `commit`, `merge-base`, or `snapshot` |
| `base` | null | Local base ref for merge-base or snapshot comparison |
| `head` | `HEAD` | Commit or head ref to analyze |
| `precision` | `tests` | `tests` permits semantic refinement; `packages` skips LSP work |
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

CI retains package-level scope and an explicit `flow-pr-v2` compatibility policy. Candidate
metadata is inspected first; artifacts and missing merge objects are read lazily in source
ancestry order. A usable record stops the Loop immediately. Incompatible or unavailable
history produces a cumulative affected comparison, while invalid current identity fails.
All comparison provenance crosses the LSP boundary through its context output.

The output is one `{"plan": ..., "validation": ...}` record. Local validation is null; CI
validation contains the current execution context and comparison for coverage and final recording.
The Flow never runs tests or publishes a success record. Source compilation/build-cache preparation,
Flow evidence/analysis/planning, and actual test execution are separate costs.

## Explicit real-server verification

The real-server smoke test is ignored by ordinary suites. CI and nightly coverage explicitly select
the pinned server and run this check alongside the protocol fixtures. To run it locally, select a
server from the pinned development environment:

```sh
nix develop --command env MF_LSP_SMOKE_SERVER=rust-analyzer \
  cargo nextest run -p mfn-lsp --test smoke --run-ignored all
```

CI invokes this same Flow and consumes package-only plans. Local semantic fallback is decided
after analysis, so some local full-suite plans still pay for LSP work. Measure compilation,
initialization, evidence discovery, analysis/planning, and script build/list/test time separately.
The policy identifier and validation record version live in the `validation_policy` constant.
