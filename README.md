# miniflow

[![CI](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml/badge.svg)](https://github.com/linw1995/miniflow/actions/workflows/CI.yaml)
[![codecov](https://codecov.io/github/linw1995/miniflow/graph/badge.svg?token=AZZ4U2PD3T)](https://codecov.io/github/linw1995/miniflow)

miniflow compiles declarative workflows into standalone executables. Each graph is a DAG; structured Loop nodes can run an inner DAG repeatedly with typed state. Ordinary nodes are statically linked Rust plugins. The core package registers the Loop container; the execution engine manages its scoped state and control steps. The compiler validates each scope and generates direct node calls in deterministic order.

## Quick start

With `mf` installed and its matching support packages available on crates.io, compile and run the [one-node example](examples/hello-workflow.json) from the repository root:

```sh
mf compile examples/hello-workflow.json --output ./hello-workflow
./hello-workflow
```

Expected output:

```json
{"answer":42}
```

On Linux or macOS, run the same executable in the terminal UI:

```sh
mf run ./hello-workflow --tui
```

If those packages are not yet available, use the [repository development commands](docs/compiling.md#repository-development), which select local support crates. The generated executable runs without the definition or build tools.

## Documentation

- [Compiling workflows](docs/compiling.md): CLI build, usage, and build failures.
- [Workflow definitions](docs/workflows.md): JSON format, built-in nodes, and validation.
- [Loop example](examples/loop.json): bounded refinement with persistent state.
- [Node development](docs/node-development.md): node registration and dependency selection.
- [Observation contracts](docs/observability.md): lifecycle schema, identity, loss semantics, and package boundaries.
- [Contributing](CONTRIBUTING.md): development setup, checks, and submission conventions.
- [Release prerequisites](docs/releases.md): support package preparation and availability checks.
- [Dependency license audit](docs/licensing.md): reports and distribution notices.

## License

miniflow is licensed under [Apache-2.0](LICENSE).
