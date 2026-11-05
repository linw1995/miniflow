# Embedding workflows in Rust programs

Use the `mf-compiler` build helper and `embed_workflow!` macro to include workflows in an existing
application or library. Embedding has two stages: the build packs the root definition and explicitly
listed source files, checking the root's syntax and structural cycles without node providers. The host
then resolves declared child Flows and plans with its registry. Preparation validates actual node
configuration, inferred ports, and execution contracts. Runtime planning and execution never read
workflow or generated source files.

## Cargo setup

Declare node and built-in Flow providers in normal dependencies only. Build scripts need no provider
registrations or `NodeConfig` metadata. Enable compiler codegen only in build dependencies:

```toml
[dependencies]
mf-compiler = { version = "=0.1.0", default-features = false }
mf-runtime = "=0.1.0"
core_nodes = { package = "mfn-core", version = "=0.1.0" }

[build-dependencies]
mf-compiler = { version = "=0.1.0", default-features = false, features = ["codegen"] }
```

Cargo and the host lockfile select packages and features. Dependency descriptors in workflow JSON
retain their existing format but do not install packages, resolve paths, or override the host graph.
No provider alias mapping is required. Link registrations in the host with ordinary Rust imports,
such as `use core_nodes as _;`. This policy also applies to Call, Loop, Iteration, and custom providers
with Flow-valued configuration: only the host registry supplies the receiving registration's declared
Flow parameters. Built-in children use the same declared-parameter authority as file and inline children.

## Generate source during the build

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    mf_compiler::embed_workflows(&[
        mf_compiler::EmbedRequest::new("answer", "workflows/answer.json")
            .with_source("workflows/child.json"),
    ])?;
    Ok(())
}
```

The root path and every `with_source` path are relative to `CARGO_MANIFEST_DIR`; absolute paths must
remain within that package directory. Symlinked sources and paths escaping the package are rejected.
In this example, a root reference to `child.json` resolves relative to `workflows/answer.json`, while
`with_source("workflows/child.json")` explicitly packs that resource. Repeat `with_source` for every
additional UTF-8 source file required by any nested child; the root is included automatically. Only
the root is parsed during packaging; other sources are parsed when an authorized field references
them in the host. The helper does not scan JSON fields for paths, discover children, or collect whole
directories.

Submit all workflows in one batch. Names must be ASCII Rust module identifiers, excluding keywords,
raw identifiers, and `_`. Invalid or duplicate names and root syntax or structural-cycle errors fail
before modules from a prior successful build are replaced.

Each workflow emits one file, `OUT_DIR/mf-embedded/<name>/workflow.rs`, containing a
`FlowSourceBundle` and host planning/preparation entry points. This build-product handoff contains
only the package-relative entry and UTF-8 resources, never a partial `ResolvedWorkflow` or selected
node contracts. The helper neither loads provider metadata nor constructs nodes or starts a nested
Cargo build. Generated `plan(registry)` resolves that bundle with the host registry and passes the
result to `plan_resolved_definition`. Configured node contracts and ports are validated during
preparation, not by Rust type checking.

The helper emits `cargo:rerun-if-changed` for the root and every explicitly supplied source. Retain
tracking for other inputs when calling it from an existing build script. Keep these files in the host
package because consumers compile its build script. Node source/feature changes follow ordinary Cargo
dependency tracking; host planning and preparation use current registration metadata.

## Prepare and execute tasks

```rust
use core_nodes as _;

mf_compiler::embed_workflow!(pub mod answer);

fn run() -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let flow = answer::prepare_workflow(&registry, None)?;
    Ok(mf_runtime::FlowRuntime::default().execute(&flow)?)
}
```

Preparation returns a `Flow` and preserves errors through `mf_compiler::PlanError`. Inspect typed
sources for node construction or contract failures. Each preparation constructs independent nodes
and prepares layouts; repeated execution of the same flow retains its node state and reuses layouts.
There is no hidden prepared-instance cache.

Use `RuntimeOptions` and existing input/context APIs. For a workflow exposing `request.text`:

```rust
let arguments = mf_runtime::WorkflowArguments::from_json(
    br#"{"request.text":"hello"}"#,
)?;
let mut context = mf_runtime::ExecutionContext::default();
context.set_workflow_arguments(arguments);
let outputs = runtime.execute_in_context(&flow, &mut context)?;
let intermediate = context.output("request.result")?;
```

Embedding uses ordinary dynamic preparation and execution, including typed node registrations,
intermediate outputs, Loop, and Iteration. Standalone runner optimizations that elide intermediate
outputs are unchanged and are not enabled for embedding.

## Prepare streams

Use `prepare_stream` for workflows whose resolved contracts require streaming:

```rust
mf_compiler::embed_workflow!(pub mod events);

fn run_events(registry: &mf_runtime::NodeRegistry) -> Result<(), Box<dyn std::error::Error>> {
    let prepared = events::prepare_stream(registry)?;
    let instance = mf_runtime::FlowRuntime::default()
        .start_stream(prepared, mf_runtime::StreamOptions::default())?;
    while let Some(message) = instance.recv()? {
        consume(message.outputs);
    }
    instance.join()?;
    Ok(())
}
```

Here `consume` is the host's handler. Drain deliveries before `join`. Starting consumes the prepared
stream; prepare again for another instance. Supply arguments, stdin, observation, and snapshots
through `StreamOptions`; use `TextInput::new(file)` for explicit stdin and the cancellation handle
to stop a running instance. Generated modules do not claim global stdio or initialize exporters.

## Plans, manifests, and observation

Every module exposes `plan(&NodeRegistry) -> Result<CompiledWorkflow, PlanError>`. It resolves the
`FlowSourceBundle` into a host `ResolvedWorkflow`, then calls `plan_resolved_definition` once. Declared
child Flows come from packed resources and the host's built-in catalog before structural planning.
The selected receiving registrations determine which configuration fields are executable children;
ordinary provider data is not interpreted as a Flow source. Planning uses registration metadata without
constructing nodes and supports existing description and observation APIs.

The plan freezes unified configuration parameter types for reachable node kinds. `NodeConfig`
derives this metadata for every field: ordinary data remains opaque `Value`, while executable fields
are `Flow` or `Optional(Flow)`. Missing optional values retain their types, and plain factories have
explicit empty metadata. There is no separate Flow-name constant or declaration list.

Offline descriptions and generated code select executable fields by scanning these frozen types;
configuration keys and scope-order paths never establish field roles. Scope orders remain separate
execution prerequisites. Saved-plan and generated preparation validate the complete receiver type
metadata before constructing providers, not merely the set of present children.

Manifest payload version `2026-11-04` requires the frozen parameter types; framing remains version 1.
Older payloads and saved plans without types are rejected rather than inferred from JSON or
order paths. This is an explicit format change, not a legacy-format adapter.

`manifest(&registry) -> Result<WorkflowManifest, PlanError>` derives the current configured interface
from the host registry. Inspection prepares separate nodes without executing them. There is no
frozen build-time schema or comparison between build and target node features. Node metadata must
obey the existing preparation contracts and remain stable for the same configuration.

Start task observation from the structural plan before fallible preparation:

```rust
let observation = answer::plan(&registry)?.start_observation(
    &observer,
    mf_telemetry::identity::RunId::new(),
)?;
let outputs = mf_runtime::ExecutionContext::run(Some(observation), |context| {
    let flow = answer::prepare_workflow(&registry, context.observation_mut())?;
    Ok::<_, Box<dyn std::error::Error>>(runtime.execute_in_context(&flow, context)?)
})?;
```

The host supplies `observer` and its telemetry dependencies. For streaming preparation observation,
use the structural plan with existing `mf_compiler::start_stream` and `StreamOptions` APIs.
Manifests are module data, without standalone executable sections; a multi-workflow host is not
automatically a runner accepted by `mf run --tui`.

## Repository example

The [embedded-workflows example](../examples/embedded-workflows) prints `{"answer":42}` and declares
`mfn-core` only as a normal dependency. From the repository root:

```sh
CARGO_TARGET_DIR="$PWD/target/embedded-example" nix develop --command \
  cargo run --manifest-path examples/embedded-workflows/Cargo.toml --offline
```

After building, the executable prepares and runs without workflow JSON, generated sources, or build tools.

## Programmatic exclusive bindings

`FlowBuilder::prepare` and `build_flow` accept marked `EdgeDefinition` values through their
existing connection argument. Set `exclusive: true` on all candidates targeting the same node
and input; no separate group list or specialized constructor is needed. All producers must
precede their consumer, and mixing ordinary and exclusive edges for one input is rejected.
Attach any control edges with `with_control_edges` before calling `into_tasks` or `into_stream`;
executable construction rejects groups whose candidates cannot be proven mutually exclusive.

Task providers can declare `NodeMetadata::exclusive_outputs`: at least two distinct declared
output names, of which at most one may produce a value per invocation. `builtin.if_else`
declares its branch outputs, including `else`. The compiler propagates these branch facts
through mandatory dependencies and retains only common facts across alternative inputs.
The contract is not supported on stateful Event or Stream nodes, whose emissions can outlive
an input invocation; their outputs do not inherit per-invocation branch facts. Loop and
iteration bodies are checked within their own scopes. Structural-only `plan_definition`
defers metadata checks, but executable preparation and code generation require proof.

Rust `EdgeDefinition` literals must initialize the new `exclusive` field; `false` preserves
ordinary binding. Prepared data connections preserve the marker through graph planning and input resolution.

Exclusive candidates use shared-value input resolution in generated and embedded execution.
Typed ownership transfer into such a consumer reports `exclusive_input` as its fallback reason;
unrelated safe segments remain eligible for optimization. Isolated node execution accepts the
already resolved input and does not run or select predecessors. Loop rounds and iteration items
resolve independently, including parallel iterations; candidate values cannot cross message domains.

## Input references and Task success prerequisites

`WorkflowDefinition` is the normalized Rust representation. Parsing version `2026-10-19` converts node-local references
into canonical data `edges`, output gates into `control_edges`, and node-success prerequisites into `order_edges`.
Serialization emits the authoring shape for the selected version, including nested bodies. Compiler entry points
therefore share one complete dependency graph; programmatic callers continue to use canonical connections.

Add node-success prerequisites to a prepared builder before executable construction:

```rust
let builder = builder.with_order_edges(vec![mf_runtime::OrderEdgeDefinition {
    from_node: "write_file".into(),
    to_node: "notify".into(),
}])?;
let flow = builder.into_tasks()?;
```

The supplied execution order must place sources before targets. Both endpoints must be ordinary Tasks in the same frame.
`TaskNode::supports_success_dependencies` defaults to true; engine controls and container providers opt out. Typed
adapters preserve the typed Task's policy. The compiler derives success activation from the source's input and output gates.
Generated runners emit direct Rust conditions; in-memory preparation binds equivalent predicates. Scheduling and output
publication establish completion, so runtime execution needs no Task outcome map. Both paths work without observation.

Compiler dependencies distinguish `Dependency::Output` from `Dependency::NodeSuccess`; compiler `FlowDependency` owns names and
`ExecutionDependency` borrows them. Executable runtime dependencies contain only data and output gates. Success prerequisites carry no input or output port. They retain activation facts,
but router success alone adds no branch-selection fact. Success errors preserve their typed source chains, and
isolated-node execution still accepts resolved inputs without running predecessors.

Execution is inferred when the host registry resolves node contracts. Embedded modules expose both
`prepare_workflow` and `prepare_stream`; finite preparation rejects graphs that require streaming.
Standalone Cargo runners resolve this choice during their build and compile one fixed execution path.

## Referenced Flow sources

`embed_workflows` packs only the root and files explicitly added with `EmbedRequest::with_source`.
It does not resolve local or built-in children at build time. Host planning resolves relative child
paths against the file that declares them, using only the packed resources. Every file resource must
be listed explicitly, including transitive children. Built-in definitions have no source directory;
they retain the existing requirement to use inline or built-in children rather than local file paths.
A missing resource is an error even if the file still exists on the host filesystem: there is no
runtime filesystem fallback. Unused packed files do not become executable children.

The host links all required node and built-in Flow providers through its normal Cargo dependencies.
Only the receiving registration's declared Flow parameters authorize child resolution; JSON key
names or source-looking ordinary data do not. Child interfaces are derived from current host contracts,
not frozen by the build helper. Provider source declarations remain provenance in this mode: embedding
does not resolve provider paths or merge provider requirements into Cargo. This also applies to
providers used by nested bodies.

For dynamic embedding, load definitions containing local Flow references with
`mf_compiler::load_embedded_workflow(path)?.definition`, then use `compile_resolved_definition`
with the host registry. Provider declarations retain the same host-owned dependency policy.
`load_workflow` instead prepares sources and provider paths for standalone compilation. Calls use the same preparation, input binding,
shared values, and isolated scope contracts in both execution backends.

Built-in Flow definitions are registered by their owning packages through
`mf_runtime::register_builtin_flow!`. The macro captures the calling Cargo package, and the registry
qualifies its local Flow name automatically. Link those packages in the host, not the build script;
`plan(&registry)`, `manifest(&registry)`, and preparation resolve built-in names from the host catalog.
For example, `mf-chat` provides `mf-chat.chat-title` from its own `workflows/title.json`; the compiler
does not depend on the chat package. The chat build still generates `title.json`, filling portable
provider requirements and feature policy from Cargo's resolved manifest declarations, including
workspace inheritance. The host registers that generated definition. No duplicate build-only
registration is needed. Source factories return definitions without executing workflow work.

## Forwarding finite child emissions

`Flow::with_stream_outputs` installs compiler-resolved `SubgraphStreamOutput` bindings. Each binding
retains its child node index, exact source `PortSpec`, parent-facing alias, and exported optionality. `stream_output_ports`
projects the containing node's emission contract. Shared values and sensitivity markers survive projection.

`Flow::execute_with_sink` executes a connected scope in order through its normal input resolution,
observation, publication, and domain-output merging. It borrows the sink synchronously and does not spawn
a forwarding worker or add a queue. Disconnected sinks use the ordinary execution path. The compiler uses
`with_input_guards` to bind success prerequisites once; dynamic execution, generated execution, and stream
forwarding all consume those guards. Typed generated-domain optimizations retain their normal fallback.
