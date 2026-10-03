# Compiling workflows

## Build the CLI

```sh
nix build .#miniflow
```

The executable is available at `./result/bin/mf`. Distributed CLI builds resolve exact-version `mf-runtime`, `mf-compiler`, and `mf-telemetry` packages from crates.io. These packages must be available before distributing the corresponding CLI release. No source-checkout discovery is performed.

## Compile and run

With Cargo, a compatible Rust toolchain, CMake, a C compiler, and the declared node dependencies available:

```sh
mf compile flow.json --output ./flow
./flow
./flow --describe
mf run ./flow --tui
mf compile flow.json --output ./flow --locked
```

The output directory must exist. A successful first build creates `flow.lock`. See [workflow definitions](workflows.md) for dependency sources, features, and lock behavior. Native libraries required by plugins must be installed separately.

Use `mf compile flow.json --output ./flow --no-telemetry` for a smaller runner when live observation is unnecessary.
It retains workflow execution, `--validate`, and `--describe`, and omits the runner's OTLP SDK and HTTP client.
This runner does not emit the lifecycle events used by `mf run --tui`. Omit `--no-telemetry` to keep observation support.
Generated runners disable `mf-compiler`'s `codegen` feature; the compiler CLI enables that feature by default.

The CLI checks graph structure in every scope and generates fixed node orchestration, including structured repeated execution for Loop bodies, then compiles one runner
using `cargo build --release --locked`. It invokes that binary with `--validate` to check registered
kinds, configuration, inferred port types, and port contracts without executing node operations.
Only the validated binary is installed. A constant such as `[1, "x"]` connected to a `List(Int64)`
input fails validation at `/1`, even on an inactive branch. Plugin errors can therefore be reported
after Rust compilation.

The generated executable embeds the graph metadata and node configuration. Node configuration is emitted as Rust string
literals in the generated workflow source. Normal execution resolves type metadata along fixed bindings, then uses
generated node calls and prints selected outputs as JSON. It does not require the definition, lock, plugin sources, or
Cargo at runtime. It retains `--validate` for
checking its embedded configuration without executing the workflow. `--describe` prints one date-versioned JSON
document containing the workflow identity, node IDs/kinds, named data/control edges, and execution order. Loop-capable runners use description version `2026-09-29` and also describe each nested body's path and local graph. It reads
the embedded plan without constructing plugins, and excludes configuration and business values. Edge names identify
connected ports; the complete list of dynamic or unconnected ports is unavailable in this description.

The runner exports workflow spans and lifecycle events over OTLP/HTTP protobuf when a collector endpoint is configured:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 ./flow
```

`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` and `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` can select signal-specific URLs. A signal
without its own URL uses `OTEL_EXPORTER_OTLP_ENDPOINT` when present; otherwise that signal opens no export connection.
The standard `OTEL_EXPORTER_OTLP_HEADERS` and signal-specific header variables are handled by the OTel exporter. The
runner uses HTTP protobuf even when an inherited protocol variable requests another transport. `MF_RUN_ID` may supply a
canonical lowercase UUID v4; otherwise each observed invocation generates one.

Without an endpoint, normal execution opens no telemetry connection. The runner buffers up to 1,024 records per signal,
uses a 100 ms batch delay with batches of up to 128, and applies a two-second HTTP timeout. On handled success or failure,
each provider gets a two-second shutdown deadline. Export failures are reported on stderr while workflow output and exit
status follow the workflow result. Existing compiled binaries must be rebuilt to gain `--describe` and export support.
Rebuild without `--locked` once to refresh an older adjacent Flow lock for the new support dependencies.

## Repository development

The `development-support` Cargo feature uses support crates from this checkout by default. Set `MF_DEV_SUPPORT_ROOT` only to override the crates directory:

```sh
nix develop
output_dir=$(mktemp -d)
cargo run -p mf-cli --features development-support -- \
  compile examples/hello-workflow.json --output "$output_dir/hello-workflow"
"$output_dir/hello-workflow"

cargo run -p mf-cli --features development-support -- \
  compile examples/cel-scalar.json --output "$output_dir/cel-scalar"
"$output_dir/cel-scalar"

cargo run -p mf-cli --features development-support -- \
  compile examples/cel-list.json --output "$output_dir/cel-list"
"$output_dir/cel-list"

cargo run -p mf-cli --features development-support -- \
  compile examples/loop.json --output "$output_dir/loop"
"$output_dir/loop"
```

Normal release builds ignore this environment variable. The hello example declares a local `mfn-core`
dependency. Both CEL examples also declare a local `mfn-code` dependency for `builtin.code`; they
produce `{"doubled":42}` and `{"doubled":[2,4]}` respectively. The Loop example declares `mfn-core`
for its Loop declaration and initial constant, and `mfn-code` for its body transformation; it produces
`{"count":3}`. A portable Flow should declare available registry or Git packages instead.

## Build failures

Diagnostics identify the failed stage: input parsing, graph validation, dependency resolution, runtime compatibility, compilation, runner validation, lock persistence, or installation. Cargo diagnostics remain visible. Failures after project creation report its retained location for inspection, including `Cargo.toml`, `workflow-plan.json`, generated source, and build artifacts.

A failed build never replaces an existing executable. If editing a constant introduces a known type
conflict, runner validation reports the source and target ports, their types, and a nested JSON
Pointer path where applicable. The previous executable and dependency lock remain intact. Runner
validation must succeed on every invocation; a previous executable is not evidence of current
success. A lock persistence failure prevents installation. If installation fails after an unlocked
build persists its dependency lock, the error states that the lock was updated.

Building third-party Rust code executes build scripts, procedural macros, and configuration factories with the user's
permissions. Factories should limit themselves to configuration validation and construction; external I/O belongs in
`TaskNode::execute` or `StreamNode::execute`. This is a native build process, not an untrusted-code sandbox.

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

## Streaming runners

Use schema `2026-10-03` with `execution.mode` set to `stream`. Sources are explicit nodes. The repository
example declares `builtin.stdin` with `config.item_type: "int"`:

```sh
nix develop --command cargo run -p mf-cli --features development-support -- \
  compile examples/stream-batch.json --output target/stream-batch
printf '1\n2\n3\n4\n5\n' | target/stream-batch
```

The result is one object per batch, such as `{"batch":[1,2,3]}` followed by `{"batch":[4,5]}`. Results are
written as they become available, including timer flushes while stdin remains open. The explicit stdin source
accepts LF, CRLF, and a final nonempty record without a newline. Arrays remain single values. Blank, malformed,
or type-invalid records fail with their line number. EOF closes only that source.

Autonomous sources run with null stdin. For an initial file-reading node exposing `path`, pass startup values:

```sh
./read-workflow --inputs '{"read":{"path":"/data/events.jsonl"}}'
./read-workflow --inputs-file ./parameters.json
./read-workflow --describe-interface
```

Input options are mutually exclusive. Values are nested by exact node ID and port; all required values and
types are validated before node execution. JSON argument transport is limited to 1 MiB and rejects duplicate
keys. Parameter files are ordinary JSON documents. `--describe-interface` returns the configured parameter
schema and input resource requirements with the workflow identity. It prepares linked providers without
executing nodes, reading source data, or initializing telemetry, and isolates construction diagnostics on
stderr. `--describe` remains a factory-free graph operation. Inspection modes do not accept execution options.

Streaming execution reserves protocol descriptors before plugin construction and routes plugin stdout to
stderr. Private descriptors are not inherited by plugin subprocesses. Slow output backpressures production;
a broken output pipe fails execution and interrupts runtime-owned input waits. The final result must be
acknowledged before successful completion. Complete earlier output lines remain effective on failure.

New graph descriptions use version `2026-10-03` and explicitly declare execution mode, lifecycle protocol, and
interface inspection. Both ordinary and `--no-telemetry` builds retain one-build validation and installation.
The terminal launcher requires the streaming consumer added by the TUI follow-up.

### Migrate an older streaming definition

Update the version, remove `execution.input_type`, and declare a source node. For pipe-driven workflows use
`builtin.stdin` with its `item_type` configuration; connect its `item` output wherever the former synthetic
source was referenced. For host-driven workflows use a named channel source and its sender. Autonomous
producers receive their requirements through workflow parameters. The old input mechanism and its dedicated
validation rules are removed; fields and graph endpoints follow the normal schema and graph rules.
