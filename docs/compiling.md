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

The generated executable embeds the graph metadata and node configuration. Node configuration is emitted as Rust string literals in the generated workflow source. Normal execution uses generated node calls and prints selected outputs as JSON. It does not require the definition, lock, plugin sources, or Cargo at runtime. It retains `--validate` for checking its embedded configuration without executing the workflow.

## Repository development

Before support packages are published, repository development uses an explicit source override enabled only by the `development-support` Cargo feature:

```sh
nix develop
output_dir=$(mktemp -d)
MF_DEV_SUPPORT_ROOT="$PWD/crates" cargo run -p mf-cli --features development-support -- \
  compile examples/hello-workflow.json --output "$output_dir/hello-workflow"
"$output_dir/hello-workflow"

MF_DEV_SUPPORT_ROOT="$PWD/crates" cargo run -p mf-cli --features development-support -- \
  compile examples/cel-scalar.json --output "$output_dir/cel-scalar"
"$output_dir/cel-scalar"

MF_DEV_SUPPORT_ROOT="$PWD/crates" cargo run -p mf-cli --features development-support -- \
  compile examples/cel-list.json --output "$output_dir/cel-list"
"$output_dir/cel-list"
```

Normal release builds ignore this environment variable. The example declares local built-in packages; a portable Flow should declare available registry or Git packages instead.

## Build failures

Diagnostics identify the failed stage: input parsing, graph validation, dependency resolution, runtime compatibility, compilation, runner validation, lock persistence, or installation. Cargo diagnostics remain visible. Failures after project creation report its retained location for inspection, including `Cargo.toml`, `workflow-plan.json`, generated source, and build artifacts.

A failed build never replaces an existing executable. Runner validation must succeed on every invocation; a previous executable is not evidence of current success. A lock persistence failure prevents installation. If installation fails after an unlocked build persists its dependency lock, the error states that the lock was updated.

Building third-party Rust code executes build scripts, procedural macros, and configuration factories with the user's permissions. Factories should limit themselves to configuration validation and construction; external I/O belongs in `Node::execute`. This is a native build process, not an untrusted-code sandbox.

## Reuse build directories

By default, builds reuse an application-owned directory under the platform's user cache root (`~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` on Linux, and `%LOCALAPPDATA%` on Windows). Its identity includes the canonical Flow path, CLI version, and generated-project layout. Changing only the output destination does not select another directory.

To control its location, including a reusable temporary directory:

```sh
mf compile flow.json --output ./flow --build-dir /tmp/order-build
mf compile flow.json --output ./flow --build-dir /tmp/order-build --locked
```

The CLI reports whether it created or reused the directory. It retains the generated Cargo project and `target` artifacts after success and failure. Identical generated files keep their modification times. Cargo decides which artifacts remain fresh; each invocation still runs validation before installation. Graph, configuration, dependency, feature, local source, and compiler changes are checked on every build.

An explicit directory must be absent, empty, or owned by the same Flow and compatible CLI/layout version. Concurrent use
reports a retry diagnostic. Definitions, dependency locks, and final output paths cannot be inside the managed
directory. Generated `src` and `target` directories must not be symbolic links. Cached generated files and working locks
are materialized independently of symbolic links or Unix hard links before reuse. Do not edit generated files as project
inputs; the next build resynchronizes them from the Flow.

Retained directories contain embedded configuration and diagnostics and are created with user-private permissions. To reclaim space, delete an inactive build directory using its reported path; never remove a directory while a build is running. A later invocation recreates it from the definition and lock. Automatic eviction and sharing compiled targets across different Flows are not provided.
