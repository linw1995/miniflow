# test-affected

Repository developer tooling for [issue #91](https://github.com/linw1995/miniflow/issues/91).
The unpublished `mfn-lsp` and `mfn-test-affected` packages share workspace checks but are not
product release dependencies, support packages, or bundled examples.

The planned Flow is `git.diff` (Task) -> `lsp.analyze` (Stream) -> `test_affected.plan` (Task).
It produces a test plan; a separate script consumes that plan. LSP servers are explicitly configured,
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
and `profile` (`standard` by default, or `rust_analyzer` for its explicit readiness/related-test
extensions). Limits default to 120000 ms, 2048 requests per snapshot, and 16 MiB per result budget.
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

`git.diff` accepts `repository`, `base`, optional `head` (default `HEAD`), optional absolute
`artifacts`, and optional `options`. It resolves local revisions and compares their merge base
with head. Snapshots are detached Git worktrees under `target/test-affected/snapshots/` by default;
they remain available after the Flow finishes. Missing refs are errors, not automatic fetches.
The node's configured `initialization_options` are forwarded to analysis without Rust-specific policy.

`test_affected.plan` consumes only the analysis output and its opaque comparison context. It uses
Cargo metadata for all declared dependency kinds, retaining full fallback for shared inputs,
dependency configuration, and analyzer failures. Source diffs remain the impact seed even when
call edges do not change. LSP evidence alone never excludes unobserved tests.

The initial narrowing rule accepts directly declared private `#[test]` functions in conventional
library modules or integration targets when only their bodies changed. It checks unchanged surrounding
code, unique test identifiers, known test macros, and absence of out-of-line children or visible shared
resource access in the test scope. Unknown patterns retain package scope. This is a deliberately limited
source ownership rule, not a proof of arbitrary program effects or test independence.

The plan records source/tree/toolchain and build-environment identity, conservative `baseline_packages`,
retained whole `packages`, exact `tests` (`package`, `binary`, `name`), and reasons. Counts remain null
without a supplied matching inventory. Options accept `configuration` (`target`, `all_features`,
`features`, `no_default_features`) and an optional `inventory` path. Default test configuration is
all features on the host target. No tests or test builds run inside the Flow.

The separate script validates identities, builds/lists the baseline once, validates exact cases,
and reuses the build for execution with nextest's default profile and no user configuration file.
Missing exact names widen to baseline. Empty plans invoke no test command; compile/test failures
preserve a nonzero exit code. The script accepts either a plan object or the Flow's `{"plan": ...}` output.

```sh
nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json --list-only --inventory-out target/test-inventory.json
nix develop --command python3 tools/test-affected/run-tests.py target/test-plan.json
nix develop --command python3 -m unittest discover -s tools/test-affected -p test_runner.py
```

Keep plans and inventories under ignored `target/`. Do not remove referenced snapshots until their plans
are no longer needed. Cleanup uses normal `git worktree remove` on explicitly chosen owned snapshots.
Changing a snapshot or compilation configuration requires a new plan. The existing coverage script
accepts package-only selection and must not be used to silently discard exact-case filters.
