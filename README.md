# miniflow

[![CI](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml/badge.svg)](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml)
[![codecov](https://codecov.io/github/linw1995/miniflow/graph/badge.svg?token=AZZ4U2PD3T)](https://codecov.io/github/linw1995/miniflow)

miniflow compiles a declarative DAG workflow into a standalone executable. Each node is a statically linked Rust plugin. The compiler validates the graph and generates direct node calls in topological order.

## Quick start

From the repository root, enter the pinned development environment and compile the [example workflow](examples/hello-workflow.json):

```sh
nix develop
output_dir=$(mktemp -d)
MF_DEV_SUPPORT_ROOT="$PWD/crates" cargo run -p mf-cli --features development-support -- \
  compile examples/hello-workflow.json --output "$output_dir/hello-workflow"
"$output_dir/hello-workflow"
```

Expected output:

```json
{"answer":42,"greeting":"hello"}
```

Flows declare their node dependencies directly in JSON. Installed release CLIs build available registry, Git, or local node packages without a source checkout; Cargo and a compatible Rust toolchain are required. The command above explicitly uses local support crates for repository development. Generated executables run without the definition or build tools.

## Documentation

- [Compiling workflows](docs/compiling.md): CLI build, usage, and build failures.
- [Workflow definitions](docs/workflows.md): JSON format, built-in nodes, and validation.
- [Node development](docs/node-development.md): node registration and dependency selection.
- [Observation contracts](docs/observability.md): lifecycle schema, identity, loss semantics, and package boundaries.
- [Contributing](CONTRIBUTING.md): development setup, checks, and submission conventions.
- [Release prerequisites](docs/releases.md): support package preparation and availability checks.
- [Dependency license audit](docs/licensing.md): reports and distribution notices.

## License

miniflow is licensed under [Apache-2.0](LICENSE).
