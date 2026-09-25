# miniflow

[![CI](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml/badge.svg)](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml)
[![codecov](https://codecov.io/github/linw1995/miniflow/graph/badge.svg?token=AZZ4U2PD3T)](https://codecov.io/github/linw1995/miniflow)

miniflow compiles a declarative DAG workflow into a standalone executable. Each node is a statically linked Rust plugin. The compiler validates the graph and generates direct node calls in topological order.

## Compile and run the example

Run these commands from the repository root. `mktemp` provides a clean output directory. Cargo builds the `mf` CLI and the generated workflow runner; the runner prints selected outputs as JSON.

```sh
nix develop
output_dir=$(mktemp -d)
cargo run -p mf-cli -- compile examples/hello-workflow.json --output "$output_dir/hello-workflow"
"$output_dir/hello-workflow"
```

Expected output:

```json
{"answer":42,"greeting":"hello"}
```

The generated executable embeds node configuration and does not need the JSON definition or the source checkout to run.

Compiling a new executable currently requires the source checkout because the generated Cargo project uses local path dependencies. Run `mf compile` from the checkout, or keep the definition inside it. The output directory must already exist. Cargo diagnostics are streamed to the terminal. A failed build leaves the existing output file intact and keeps its generated project in an adjacent `.mf-build-*` directory for inspection.

## Workflow definition

The [example definition](examples/hello-workflow.json) uses schema version `2026-09-24`. This is the only accepted version at present. Each node has a unique, nonblank string `id`, a registered `kind`, and an optional JSON `config`. An edge connects a named output port to a named input port. An `outputs` entry selects a node port and gives it a name in the executable's JSON output.

The built-in bundle currently provides:

| Kind | Configuration | Input ports | Output ports |
| --- | --- | --- | --- |
| `builtin.constant` | Required `value`: any JSON value | None | `value`: any value |
| `builtin.identity` | None | Required `input`: any value | `value`: the unchanged input |

The compiler rejects unknown kinds, invalid configuration, missing nodes or ports, incompatible port types, missing required inputs, multiply connected inputs, repeated output names, and cycles before generating a runner.

## Plugins and bundle selection

Each built-in node lives in its own crate under `crates/builtin-nodes/`. A plugin crate depends on `mf-runtime`, implements `Node::execute`, provides a factory, and submits a `NodeRegistration` through `inventory::submit!`. The registration declares a unique `kind` and its input and output `PortSpec` values. See [constant](crates/builtin-nodes/constant/src/lib.rs) and [identity](crates/builtin-nodes/identity/src/lib.rs) for working registrations.

`inventory` only collects registrations from linked crates. Workspace membership alone does not link a plugin into the compiler or generated executable.

The current plugin set is selected by [mf-bundle](crates/mf-bundle/Cargo.toml): add the node crate as a dependency there and reference its exported `kind()` in [mf-bundle's registry](crates/mf-bundle/src/lib.rs). Rebuild `mf` after changing the bundle. Both the compiler and generated runner use this bundle, so they see the same node kinds. Bundle selection is currently a build-time choice; the CLI has no bundle flag.

## Development and releases

Enter the pinned development environment and run the repository checks:

```sh
nix develop
prek install
prek -a
nix flake check -L
```

Commit messages and PR titles use the Conventional Commits header format:
`<type>(<scope>)[!]: <description>`. Supported types are `build`, `chore`,
`ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `revert`, `style`, and `test`.
The required scope must be one of `cli`, `runtime`, `compiler`, `bundle`,
`nodes`, `ci`, or `docs`. For example, `feat(compiler): validate graph inputs`
and `fix(runtime)!: reject incompatible workflows` are valid. `prek install`
installs both the pre-commit and commit-msg hooks.

To run the workspace tests under nextest and generate coverage reports locally:

```sh
nix develop --command bash scripts/run-cov.sh
```

The reports are written to `target/coverage/result/`, including `lcov.info` for Codecov.

Workspace crates share their version in the root `Cargo.toml`. Internal dependencies
declare both a local path and a registry version there, so Cargo can package the
workspace for crates.io. With Cargo 1.90 or newer, `cargo publish --workspace`
publishes the crates in dependency order. Run `cargo publish --workspace --dry-run`
to check the packages before a release.

The Nix package also exposes the CLI as `./result/bin/mf` after `nix build .#miniflow`.

Push a `release/<version>` branch to build native archives for Linux and macOS. The branch version must match `crates/mf-cli/Cargo.toml`. The CD workflow publishes a GitHub release tagged `v<version>` after all archives have been built.
