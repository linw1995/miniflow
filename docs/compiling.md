# Compiling workflows

## Build the CLI

```sh
nix build .#miniflow
```

The executable is available at `./result/bin/mf`. Distributed CLI builds resolve exact-version `mf-runtime` and `mf-compiler` packages from crates.io. These packages must be available before distributing the corresponding CLI release. No source-checkout discovery is performed.

## Compile and run

With Cargo, a compatible Rust toolchain, and the declared node dependencies available:

```sh
mf compile flow.json --output ./flow
./flow
mf compile flow.json --output ./flow --locked
```

The output directory must exist. A successful first build creates `flow.lock`. See [workflow definitions](workflows.md) for dependency sources, features, and lock behavior. Native libraries required by plugins must be installed separately.

The CLI checks graph structure and generates fixed node orchestration, then compiles one runner using `cargo build --release --locked`. It invokes that binary with `--validate` to check registered kinds, configuration, and port contracts without executing node operations. Only the validated binary is installed. Plugin errors can therefore be reported after Rust compilation.

The generated executable embeds the graph metadata and node configuration. Normal execution uses generated node calls and prints selected outputs as JSON. It does not require the definition, lock, plugin sources, or Cargo at runtime. It retains `--validate` for checking its embedded configuration without executing the workflow.

## Repository development

Before support packages are published, repository development uses an explicit source override enabled only by the `development-support` Cargo feature:

```sh
nix develop
output_dir=$(mktemp -d)
MF_DEV_SUPPORT_ROOT="$PWD/crates" cargo run -p mf-cli --features development-support -- \
  compile examples/hello-workflow.json --output "$output_dir/hello-workflow"
"$output_dir/hello-workflow"
```

Normal release builds ignore this environment variable. The example declares local built-in packages; a portable Flow should declare available registry or Git packages instead.

## Build failures

Diagnostics identify the failed stage: input parsing, graph validation, dependency resolution, runtime compatibility, compilation, runner validation, lock persistence, or installation. Cargo diagnostics remain visible. Failures after project creation report its retained location for inspection, including `Cargo.toml`, `workflow-plan.json`, generated source, and build artifacts.

A failed build never replaces an existing executable. Runner validation must succeed on every invocation; a previous executable is not evidence of current success. A lock persistence failure prevents installation. If installation fails after an unlocked build persists its dependency lock, the error states that the lock was updated.

Building third-party Rust code executes build scripts, procedural macros, and configuration factories with the user's permissions. Factories should limit themselves to configuration validation and construction; external I/O belongs in `Node::execute`. This is a native build process, not an untrusted-code sandbox.
