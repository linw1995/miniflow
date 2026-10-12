# Verification

## Focused validation

All focused commands ran in the pinned `nix develop` toolchain and passed:

- `cargo test -p mf-runtime --test node_arguments --test runner_arguments`: selection, input/context
  transport, typed errors, regular-file/FIFO handling, presence semantics, and canonical size boundaries.
- `cargo test -p mf-compiler --test node_targets --test single_node_execution --test dependency_project`:
  preparation reuse and drift, isolation, composites, resource bounds, publication, identity, lifecycle,
  snapshots, and generated project structure.
- `cargo test -p mf-compiler --test single_node_runner -- --test-threads=1`: finite and streaming binaries,
  telemetry feature combinations, noisy factories, unsupported targets, failures, portability, and typed
  serial segments.
- `cargo test -p mf-cli --test runner_observability -- --test-threads=1`: real OTLP lifecycle and snapshot
  reception for selected operations alongside existing whole-flow exporter regressions.
- The documented CEL selection command returned `{"doubled":60}` for `--transform.amount 30`.

## Repository validation

- `nix develop --command prek install` installed both required hooks.
- `nix develop --command prek -a` passed all repository hooks, including Clippy, Cargo check, rust-analyzer,
  Markdown/YAML checks, workflow lint, and the dependency license audit.
- `openspec validate execute-single-node-in-compiled-runner --strict --no-interactive` passed.
- `nix develop --command bash scripts/run-cov.sh --no-fail-fast` completed with 582 passing tests and two
  existing skipped tests. Reports are local under `target/coverage/result/`, including `lcov.info`, HTML,
  Cobertura, and Markdown. Target execution line coverage is 88.5%; runner argument parsing is 90.5%.
- `nix flake check -L` passed on `aarch64-darwin`. Its release CI test profile completed with 582 passing tests and two existing skipped tests on
  `aarch64-darwin`. Build, formatting, Clippy, and workflow checks also passed their validation phases.

## Archive validation

- `openspec archive execute-single-node-in-compiled-runner --yes --json` synchronized the main specs and
  archived this change as `2026-10-12-execute-single-node-in-compiled-runner`.
- All 22 main specifications passed ordinary validation. `single-node-execution` and
  `workflow-runtime-execution` passed strict validation. Comparing full strict findings with the recorded
  pre-archive baseline found no new warnings or errors.
- This archive passed validation with all 27 tasks completed. Full archive validation reported one existing
  failure: `2026-10-05-unify-flow-runtime` has 17 incomplete tasks. Its task file is identical to the version
  before this change's archival and remains outside this implementation's scope.

## Integration with current main

- Integrated `0e793e9` from main, retaining its verbose execution and CI timing changes.
- Selected invocations use the existing verbosity parser and local stderr observer. Regression cases cover
  flag-like string values, accumulated levels, finite operation counts, and selected Tasks in both finite
  and streaming binaries, with and without the telemetry SDK.
- Focused parser, generated-project, standalone-runner, and real OTLP suites passed all 19 tests in the
  pinned toolchain. `prek -a` and ordinary validation of all 22 main specifications also passed.

## Delivery scope

The implemented phase supports top-level Tasks in finite and streaming flows and complete Loop/Iteration
operations. Producer, Event, body-local/control, and TUI selection remain follow-ups. Whole-plan factory
preparation remains an explicit limitation. The implemented phase was archived after the post-implementation review, ablation experiments, and
repository checks. Archival closes this phase without claiming universal arbitrary-node completion for
Issue #125.
