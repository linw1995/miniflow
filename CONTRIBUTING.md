# Contributing

## Environment and checks

Use the pinned toolchain from the repository root and install the pre-commit and commit-msg hooks:

```sh
nix develop
prek install
```

Before submitting changes, run:

```sh
prek -a
cargo run -p mf-cli --features development-support -- compile \
  tools/test-affected/flows/test-affected.json --output target/test-affected-runner \
  --build-dir target/test-affected-build
cargo run -p mf-cli --features development-support -- run ./target/test-affected-runner \
  --params.mode worktree --params.precision packages --params.run true
```

Omit `--params.run true` to produce a plan without executing tests.

These checks cover formatting, Rust diagnostics, affected package tests, workflow configuration, Markdown, and the [dependency license audit](docs/licensing.md).
The JSON hook automatically formats files with two-space indentation, preserving key order and Unicode characters.
See [test selection](docs/ci.md#affected-package-tests) for commit and PR comparisons.
Do not run `nix flake check -L` locally: its test check repeats the complete suite already run by CI coverage.

## Tests

The standard nextest suite includes packaged CLI acceptance and release prerequisite checks:

```sh
nix develop --command cargo fetch --locked
nix develop --command cargo nextest run --workspace --all-targets --all-features
```

Fetch the complete locked dependency graph before a first test run: offline fixture builds also resolve dependencies for other targets. The coverage and release workflows perform this preparation automatically.

On Unix, nextest setup prebuilds common runner dependencies before selected compiler integration tests. These tests
share a Cargo target directory and run serially through validation and execution. Setup has its own timing entry;
its duration is included in the overall run, but not in individual test durations. Cache invalidation, cold rebuild,
and packaged acceptance tests keep isolated targets. Plain `cargo test` also keeps its existing isolated builds.

This uses nextest's experimental setup scripts, with nextest and Python provided by `nix develop`.
Warmup preserves Cargo wrappers, compiler flags, and coverage instrumentation.
Each run gets its own directory under the Cargo target's `nextest-build-cache/`, so concurrent runs cannot overwrite
each other's runners. These build artifacts remain until removed or cleaned with `cargo clean`.

Coverage CI sets `MF_TEST_BUILD_CACHE_DIR` to a stable, cached directory within its isolated runner.
This retains vendored source paths and both runner dependency profiles between runs. Setup refreshes
the vendor graph and cleans workspace packages before rebuilding, so restored artifacts cannot hide
checkout changes. When setting this variable locally, give concurrent nextest runs separate directories.

Nix checks and CI coverage use the `ci` nextest profile. It shares the prewarmed Cargo target across additional generated-runner tests while serializing access to their executable. The default local profile retains more test parallelism. To reproduce CI scheduling locally, run `nix develop --command cargo nextest run --profile ci --workspace --all-features`.

CI's Linux `check` job reports formatting and workflow validation separately from package builds and Clippy.
Package builds and Clippy share one Nix invocation so they can run in parallel and reuse dependencies.
The `coverage` job runs affected package tests on PRs and the complete Linux suite on `main`.
Its PR job summary explains the selected packages or full-workspace fallback. Main-only macOS jobs
build the package and run static checks; nightly runs the complete macOS suite.

After a PR's CI run completes, the independent `CI timing monitor` workflow updates a single bot comment
with job and step durations. Its baseline is the median of the latest five successful CI pushes to the
PR's base branch completed before that run started. Only matching jobs contribute to the wall time,
including queueing and dependencies; main-only macOS checks are excluded from PR comparisons.
Failed runs show their status without claiming a wall-time improvement. The monitor also writes the
report to its job summary, handles fork PRs, and ignores superseded heads and older attempts.
It executes only default-branch code and reads timings from GitHub's API. A new `workflow_run` workflow
starts receiving events after it is merged into the default branch.

To run only these integration tests:

```sh
nix develop --command cargo nextest run -p mf-cli --test packaged_cli --test release_support
```

`packaged_cli_acceptance` builds a default CLI and third-party packages, then verifies registry, pinned local Git, and
local-path dependencies outside the checkout, locked rebuilds, and standalone execution. It is enabled without a feature
flag or `--ignored`. A Rust fixture prepares the package archives and isolated Cargo source, and cleans up temporary
files after the test. Nextest captures subprocess diagnostics and reserves the test worker slots while acceptance runs
to avoid competing nested Cargo builds. Git, Bash, and jq are provided by the development environment.

Nix uses the same nextest suite. Package builds leave test execution to that check so acceptance runs once. The coverage command below also discovers these tests through nextest; there is no separate post-check invocation to maintain.

## Coverage

When coverage is relevant, run:

```sh
nix develop --command bash scripts/run-cov.sh
```

Reports are written to `target/coverage/result/`, including `lcov.info`.
Coverage dependencies are retained under `target/coverage/build/`. Each run clears raw profiles and reports and rebuilds
workspace packages so stale executable mappings cannot affect the report. A toolchain, instrumentation, or workspace
membership change resets the build cache. CI caches the dependency build directory separately from Nix; per-run nested
runner targets are excluded from that cache. Remove `target/coverage/build/` to force a completely fresh coverage build.

## Performance

Measure planning, preparation, execution, and source generation separately with a Constant/Identity chain:

```sh
nix develop --command cargo run --release -p mf-compiler --example benchmark_graph -- 4000 0
nix develop --command cargo run --release -p mf-compiler --example benchmark_graph -- 256 1048576
```

The arguments select the node count and string payload size in bytes; zero selects a scalar value.
Run the built executable repeatedly when comparing timings or measuring peak memory, so Cargo compilation is excluded.
Keep machine-specific measurements and ablation reports under the ignored `target/` directory.

Compare owned and shared snapshots for an unchanged terminal session:

```sh
nix develop --command cargo run --release -p mf-tui --example benchmark_snapshots -- 10000 1000
```

The arguments select the node count and repetition count. Shared snapshots are rebuilt when the session receives updates.

## Code layout

| Path | Responsibility |
| --- | --- |
| `crates/mf-cli/` | The `mf` command and project build orchestration |
| `crates/mf-compiler/` | Workflow validation, planning, code generation, and executable builds |
| `crates/mf-runtime/` | Workflow definitions, node interfaces, registry, and execution support |
| `crates/mf-chat/` | Persistent chat nodes and embedded application workflows |
| `crates/builtin-nodes/core/` | `mfn-core` basic node implementations |
| `crates/builtin-nodes/code/` | `mfn-code` CEL node implementation |
| `tools/test-affected/nodes/` | Unpublished developer-only LSP and affected-test providers |

See [node development](docs/node-development.md) for adding nodes. Developer tools share workspace validation but are not product release dependencies.

## Commits and pull requests

Use `<type>(<scope>)[!]: <description>` for commit messages and pull request titles. The optional `!` marks a breaking change.

- Types: `build`, `chore`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, `test`.
- Scopes: `cli`, `runtime`, `compiler`, `bundle`, `nodes`, `ci`, `deps`, `docs`.

For example:

```text
docs(docs): simplify documentation navigation
```

Keep pull request descriptions concise and explain the problem and resulting behavior.
