# Compiling workflows

To include validated workflows in an existing Rust application or library, see
[Embedding workflows](embedding.md).

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
mf run ./flow --verbose
mf run ./flow --tui
mf compile flow.json --output ./flow --locked
```

The output directory must exist. A successful first build creates `flow.lock`. See [workflow definitions](workflows.md) for dependency sources, features, and lock behavior. Native libraries required by plugins must be installed separately.

`mf run ./flow` inherits stdin, stdout, and stderr and returns the runner's exit status. It requires no terminal.
Generated Unix runners handle SIGINT and SIGTERM by cancelling the active run, including
streaming and `--node` execution. Cooperative nodes can release their resources before
the runner exits with status 130 (SIGINT) or 143 (SIGTERM). A second interrupt forces
immediate exit with the corresponding status, including during cleanup or startup.
Embedded runtime calls do not install process-wide signal handlers.
Use `-v` or `--verbose` with either `mf run ./flow` or `./flow` to print execution details on stderr:

| Option | Execution diagnostics |
| --- | --- |
| `-v` | Readable workflow/node progress, outcomes, timings, Loop passes, and stream context. |
| `-vv` | Summaries plus event name/sequence, workflow ID, and available node position/kind. |
| `-vvv` | Complete existing lifecycle `WireRecord` as compact JSON, including available trace context. |

Clustered flags and repeated `-v`/`--verbose` accumulate, with totals above three selecting the highest level.
`mf -v run ./flow -vv` selects level three. A consumed input value such as `--read.path -vv` remains a string value.
Readable summaries use elapsed and node-duration milliseconds, one-based Loop passes, full unsigned stream IDs,
and escaped identifiers/errors. The run ID appears at workflow start; full JSON retains the original record fields.
Results remain on stdout and diagnostics contain execution metadata rather than input/output values.
While a node is running, every positive level prints a readable `still running` reminder every 60 seconds. Higher
levels identify its original start event. Completion, failure, and observation cleanup stop its reminders.
These reminders also work with `--no-telemetry` and do not add OTLP lifecycle events.
Place CLI options before named workflow parameters, for example `mf run ./flow -v --read.path data.txt`.
With `--tui`, verbose runner diagnostics appear in the TUI's captured stderr history.
`mf compile flow.json --output ./flow --verbose` reports compilation options, elapsed time, and the retained project;
any positive verbosity enables that compilation summary.
Recompile existing executables to add the runner's verbose option.

Use `mf compile flow.json --output ./flow --no-telemetry` for a smaller runner when live observation is unnecessary.
It retains workflow execution and the embedded manifest, and omits the runner's OTLP SDK and HTTP client.
Verbose stderr diagnostics still work without a collector or OTLP support. This runner does not export the lifecycle
events used by `mf run --tui`. Omit `--no-telemetry` to keep live TUI observation support.
Generated runners disable `mf-compiler`'s `codegen` feature; the compiler CLI enables that feature by default.

The CLI checks graph structure in every scope and generates fixed node orchestration, including structured repeated execution for Loop bodies, then compiles one runner
using `cargo build --release --locked`. Its build script checks registered kinds, configuration, inferred
port types, and port contracts without executing node operations. Only a successfully built binary is installed.
A constant such as `[1, "x"]` connected to a `List(Int64)` input fails build validation at `/1`, even on an inactive
branch. No validation process is launched after compilation.

The generated executable embeds the graph metadata and node configuration. Node configuration is emitted as Rust string
literals in the generated workflow source. Normal execution resolves type metadata along fixed bindings, then uses
generated node calls and prints selected outputs as JSON. It does not require the definition, lock, plugin sources, or
Cargo at runtime. The embedded manifest contains a date-versioned JSON graph record with workflow identity,
node IDs/kinds, named data/control edges, and execution order. Loop-capable runners use description version
`2026-09-29` and also describe each nested body's path and local graph. The record excludes configuration and business
values. Edge names identify connected ports; the complete list of dynamic or unconnected ports is unavailable in this record.

The generated Cargo build prepares the selected providers once to produce immutable execution layouts and a versioned
workflow manifest containing the original definition, frozen execution orders, and prepared input schema. Both artifacts use the same validated preparation,
including configured input types and conditional stdin requirements. The manifest is generated before linking during
the existing runner build; it does not require a second compilation. Its wire format is documented in
[observability](observability.md#embedded-manifest-format). The generated-project layout version is `2026-10-16`;
explicit build directories owned by an older layout must be recreated at a compatible location.
The runner links the manifest into a dedicated ELF or Mach-O section and needs no manifest sidecar after installation.
Runtime helpers derive graph and interface views; generated runners use them directly for observation and inspection.
TUI preflight reads the section directly without constructing providers. Missing or corrupt manifests and unsupported
executable formats are rejected without launching an inspection process; older runners must be recompiled.
Normal execution compares freshly prepared startup declarations with the manifest before dispatch.
Changes to input identities, types, required flags, or stdin conditions fail with node/port/resource context.
Runtime initialization failures and interface drift are reported at startup, before business execution or source consumption.

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
status follow the workflow result. Existing compiled binaries must be rebuilt to gain embedded manifest and export support.
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

The terminal and multi-turn LLM examples declare `mfn-llm` and require a configured model and execution-time
credentials; their build validation performs no provider request. See
[terminal conversations](workflows.md#llm-configuration-and-conversation-values).

## Build failures

Diagnostics identify the failed stage: input parsing, graph validation, dependency resolution, runtime compatibility, compilation, lock persistence, or installation. Cargo diagnostics remain visible. Failures after project creation report its retained location for inspection, including `Cargo.toml`, `workflow-plan.json`, generated source, and build artifacts.

A failed build never replaces an existing executable. If editing a constant introduces a known type
conflict, build validation reports the source and target ports, their types, and a nested JSON
Pointer path where applicable. The previous executable and dependency lock remain intact. Cargo tracks the generated build inputs; changed definitions or providers must pass build validation
before installation. A lock persistence failure prevents installation. If installation fails after an unlocked
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

## Runner help

Newly compiled runners expose their embedded input schema through `--help` or `-h`:

```sh
./flow --help
./flow --node transform --help
```

Help lists qualified input names, types, required flags, JSON transports, and verbosity options. With
`--node`, it shows only the selected task's inputs. Help writes to stdout and exits successfully without
constructing providers, reading input/context files, or executing nodes. CLI syntax errors write to stderr
and exit with status 2; input, context, and execution validation errors remain failures.

`mf run ./flow --help` (or `-h`) combines CLI help with workflow help generated from the existing
embedded `input_schema`. It reads the binary without executing it, preparing providers, reading
input files, or entering TUI/chat mode; executable permission and a terminal are not required.
Both Linux ELF64 and macOS Mach-O64 files can be inspected, with no manifest format changes.
The native runner's Task target list comes from separate execution metadata and is omitted from
this schema-based help. `mf run --help` shows only CLI help.

Put help before runner arguments. Once a runner argument starts, the remaining arguments belong to
the runner: `mf run ./flow --node transform --help` launches the runner to display its node help.
An explicit separator, such as `mf run ./flow -- --help`, also forwards arguments directly and shows
only runner help. Missing, invalid, or unsupported manifests produce a diagnostic and a nonzero
exit status.

JSON and file transports also accept inline values, such as `--inputs='{"read.path":"data.txt"}'` and
`--inputs-file=parameters.json`. A named string value starting with `--` must use inline syntax, such as
`--read.path=--literal`; short-option-looking values such as `-v` remain literal when following a named input.

## Finite live output

New compiled finite workflows keep one final JSON object by default. Workflows declaring `stream_outputs`
can opt into the versioned business-output protocol independently of telemetry:

```sh
./flow --count.count 1 --output-format events-jsonl --stream-output increments
```

This example assumes a combined counting provider with a public `increments` export. Repeat `--stream-output NAME`
to select multiple exports; omission subscribes to all declared exports. Unknown or duplicate selections fail before
business execution. An unselected export has no host consumer and can produce zero increments. These controls are
runner options, so a workflow string such as `--read.path=--output-format` remains a literal input value.

Each stdout line is one compact JSON record. A record carries wire version `1`, the execution run ID, an ordered
sequence, and either a live event or a complete final output object. For example:

```json
{"version":1,"run_id":"0123456789abcdef0123456789abcdef","sequence":0,"record":{"kind":"event","event":{"export":"increments","source":["count"],"invocation":{"id":1,"node":"count","path":[]},"event":{"kind":"open"}}}}
{"version":1,"run_id":"0123456789abcdef0123456789abcdef","sequence":1,"record":{"kind":"event","event":{"export":"increments","source":["count"],"invocation":{"id":1,"node":"count","path":[]},"event":{"kind":"value","value":0}}}}
{"version":1,"run_id":"0123456789abcdef0123456789abcdef","sequence":2,"record":{"kind":"event","event":{"export":"increments","source":["count"],"invocation":{"id":1,"node":"count","path":[]},"event":{"kind":"close","completed":true}}}}
{"version":1,"run_id":"0123456789abcdef0123456789abcdef","sequence":3,"record":{"kind":"final","outputs":{"total":1}}}
```

Open/close controls identify each invocation even when it produces no values. A skipped node produces no controls.
The static source path preserves opaque IDs, while the dynamic invocation path preserves Loop/Iteration indices.
Closing a stream ends that production invocation; the final result remains authoritative. The final record follows
successful graph and export drain. Diagnostics and provider stdout are redirected to stderr in event mode.
Failure can end without a final record; valid prefixes must not be treated as successful replies.

The manifest and input-interface view revision `2026-10-23` expose the finite live capability and reflected export
contracts. Generated preparation validates these contracts before execution, including nested bodies and isolated
Task binding. Source paths are independent of observation's outer-only Iteration view. Pure native streaming
workflows retain their existing result-line protocol; event mode is available for their isolated Task targets.
Older JSON-only executables remain usable and must be recompiled to add the new protocol.

Default transport bounds are 1 MiB per encoded event record, 16 MiB for the final output object, and 64 MiB cumulative
wire data. The final frame has bounded framing overhead. Incremental readers also bound their partial-record buffer,
reject duplicate keys, validate run/sequence/source/lifecycle/type contracts, and reject incomplete final records.
A fully validated final value is retained if later process or protocol diagnostics arrive. Live capture retains only
that final value and bounded in-flight records rather than spooling the entire event history. Capture and cancellation
continue independently of terminal rendering.

## Streaming runners

Use schema `2026-10-03` with `execution.mode` set to `stream`. Sources are explicit nodes. The repository
example declares `builtin.readline` with string output `line`:

```sh
nix develop --command cargo run -p mf-cli --features development-support -- \
  compile examples/stream-batch.json --output target/stream-batch
printf '1\n2\n3\n4\n5\n' | target/stream-batch
```

The result is one object per batch, such as `{"batch":["1","2","3"]}` followed by `{"batch":["4","5"]}`.
Results are written as they become available, including timer flushes while stdin remains open. Readline
preserves blank lines and whitespace, accepts LF/CRLF and a final unterminated line, and rejects invalid UTF-8
with a line number. It does not interpret text as JSON. EOF closes only that source.

Autonomous sources run with null stdin. For an initial file-reading node exposing `path`, pass startup values:

```sh
./read-workflow --read.path /data/lines.txt
./read-workflow --inputs-file ./parameters.json
```

Named inputs use `--node.port VALUE` or `--node.port=VALUE`. Strings accept plain text; other types accept
JSON values. JSON transport options cannot be combined with named inputs. Parameter files use flat qualified
keys, such as `{"read.path":"/data/lines.txt"}`. All required values and types are validated before node execution. JSON argument transport is limited to 1 MiB and rejects duplicate
keys. Parameter files are ordinary JSON documents. The embedded manifest exposes the configured parameter schema
and input resource requirements with the workflow identity. Inspection reads executable metadata without preparing
providers, reading source data, or initializing telemetry.

Streaming execution reserves protocol descriptors before plugin construction and routes plugin stdout to
stderr. Private descriptors are not inherited by plugin subprocesses. Slow output backpressures production;
a broken output pipe fails execution and interrupts runtime-owned input waits. The final result must be
acknowledged before successful completion. Complete earlier output lines remain effective on failure.

New graph descriptions use version `2026-10-03` and explicitly declare execution mode, lifecycle protocol, and
interface inspection. Both ordinary and `--no-telemetry` builds retain one-build validation and installation.
Use `mf run ./workflow --tui --inputs-file ./parameters.json` to observe autonomous streams.
The TUI owns terminal stdin and launches the child with null stdin. It validates parameters and rejects
workflows that still require stdin. Supply a readline `path` through startup arguments for TUI execution;
run the executable directly to read piped text. Startup options also work with new finite runners.

### Migrate an older streaming definition

Update the version, remove `execution.input_type`, and declare a source node. For pipe-driven workflows use
`builtin.readline` and connect its `line` output wherever the former synthetic source was referenced.
Parse text explicitly downstream when typed values are needed. Application-specific producers use ordinary
StreamNode implementations. Initial producers receive their requirements through workflow parameters.
The old input mechanism and its dedicated validation rules are removed; fields and graph endpoints follow
the normal schema and graph rules.

## Execute an isolated task

The generated build emits `node-targets.json` alongside execution layouts, using the same validated
provider preparation. Each target retains its compiled index, resolved port types, external context
references, stdin conditions, and a finite operation description. `NodeTarget::bind_flow` and
`NodeTarget::bind_stream` bind a top-level task to that frozen contract and reject contract drift before
business execution. Inference still follows the original graph, including forwarding output types.

Isolated task support reuses whole-plan preparation. An unrelated provider factory can still fail during
initialization and prevent selection from running; preparation does not call business methods or consume
source data. Producers and Event executors are recorded for accurate unsupported-target diagnostics and
cannot be bound as isolated tasks. Recompile existing executables to obtain `--node` support.

The isolated runner invocation contract uses the selected task's qualified inputs:

```sh
./flow --node transform --transform.amount 21
./flow --node=transform --inputs '{"transform.amount":"21"}'
./flow --node transform --inputs-file inputs.json
./flow --node route --context-file context.json
./flow -vv --node transform --transform.amount 21
```

`--node` identifies one exact top-level Task. Named values follow ordinary string/JSON rules; use inline
syntax for a string starting with `--`. Named inputs cannot mix with JSON input transport. Required inputs
of other roots are neither required nor accepted, and optional omissions remain absent. The selected
contract is independent of definition-schema startup promotion.

Verbosity flags retain their ordinary diagnostic levels for selected execution. Lifecycle records describe
the finite selected operation and remain on stderr, including in runners compiled with `--no-telemetry`.

Context documents are flat objects keyed by the target's declared external reference IDs, such as
`{"source.value":{"count":42}}`. Supply either `--context JSON` or `--context-file PATH`, only with selection.
Every declared reference must be supplied; unrelated keys and duplicate keys are rejected. Values are
checked against the original graph's referenced output types. Null is a value, not a skipped-output marker.
Context files resolve from the invocation directory, must be regular files, and are read once with a
bounded reader. Each port/context transport and their combined canonical JSON payload must fit within
1 MiB. Context values do not execute their producers or change the target's compiled types.

A selected invocation returns one JSON object keyed by all produced output port names, independently of
workflow output aliases. Explicitly skipped ports are omitted and null values remain present. Provider
construction and execution diagnostics are routed to stderr; stdout contains only the finite result.
Failure returns a nonzero status without a successful result object. This finite behavior also applies
when the selected Task belongs to a stream flow: other producers, Event callbacks, and timers do not start.

For the repository CEL example:

```sh
mf compile examples/cel-scalar.json --output ./flow
./flow --node transform --transform.amount 30
```

The selected result is `{"doubled":60}`. A selected Loop or Iteration runs its complete prepared body,
including its usual limits, exit behavior, worker bounds, and error policy. Producer and Event selection,
body-local nodes, synthetic scope controls, and `mf run --tui --node` are not supported in this phase.
Direct manifest inspection continues to describe whole-flow startup and observation; target selection does
not expand the manifest interface. See [isolated observations](observability.md#isolated-task-observations)
for selected-operation identity, counts, and opt-in snapshots.
