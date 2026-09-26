# miniflow

[![CI](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml/badge.svg)](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml)
[![codecov](https://codecov.io/github/linw1995/miniflow/graph/badge.svg?token=AZZ4U2PD3T)](https://codecov.io/github/linw1995/miniflow)

miniflow compiles a declarative DAG workflow into a standalone executable. Each node is a statically linked Rust plugin. The compiler validates the graph and generates direct node calls in topological order.

## Quick start

From the repository root, enter the pinned development environment and compile the [example workflow](examples/hello-workflow.json):

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

Compiling workflows currently requires the source checkout and Cargo. The generated executable embeds node configuration and runs without the JSON definition or checkout.

## Documentation

- [Compiling workflows](docs/compiling.md): CLI build, usage, and build failures.
- [Workflow definitions](docs/workflows.md): JSON format, built-in nodes, and validation.
- [Plugin development](docs/plugins.md): node registration and bundle selection.
- [Contributing](CONTRIBUTING.md): development setup, checks, and submission conventions.
- [Dependency license audit](docs/licensing.md): reports and distribution notices.

## License

miniflow is licensed under [Apache-2.0](LICENSE).
