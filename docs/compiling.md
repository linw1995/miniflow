# Compiling workflows

For a first run, follow the [quick start](../README.md#quick-start).

## Build the CLI

From the repository root:

```sh
nix build .#miniflow
```

The executable is available at `./result/bin/mf`.

## Compile and run

Compiling a new executable requires Cargo and the source checkout because the generated Cargo project uses local path dependencies. Run `mf compile` from the checkout, or keep the definition inside it. The output directory must already exist.

From the repository root, after building the CLI:

```sh
nix develop
output_dir=$(mktemp -d)
./result/bin/mf compile examples/hello-workflow.json --output "$output_dir/hello-workflow"
"$output_dir/hello-workflow"
```

The compiler validates the [workflow definition](workflows.md), generates direct node calls in topological order, and builds the runner with `cargo build --release`. The runner prints selected outputs as JSON.

The generated executable embeds node configuration and does not need the JSON definition or source checkout to run. For distribution requirements, see [dependency license auditing](licensing.md).

## Build failures

Cargo diagnostics are streamed to the terminal. A failed build leaves the existing output file intact and keeps its generated project in an adjacent `.mf-build-*` directory for inspection. Successful builds remove that temporary directory.
