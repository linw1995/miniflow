# Embedding workflows in Rust programs

Use the `mf-compiler` build helper and `embed_workflow!` macro to include workflows in an existing
application or library. The build checks definition structure and emits Rust source. Preparation
uses the host registry to validate actual node configuration, inferred ports, and execution contracts.
Execution uses existing runtime APIs without reading workflow or generated source files.

## Cargo setup

Declare node crates only in normal dependencies. Enable compiler codegen in build dependencies and
disable it in the normal dependency:

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
No provider alias mapping or node build dependencies are required. Link node registrations in the
host with ordinary Rust imports, such as `use core_nodes as _;`.

## Generate source during the build

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    mf_compiler::embed_workflows(&[
        mf_compiler::EmbedRequest::new("answer", "workflows/answer.json"),
    ])?;
    Ok(())
}
```

The helper accepts definitions relative to `CARGO_MANIFEST_DIR`, or absolute paths. Submit all
workflows in one batch. Names must be ASCII Rust module identifiers, excluding keywords, raw
identifiers, and `_`. Invalid or duplicate names and invalid graph structure fail before modules
from a prior successful build are replaced.

Each workflow emits one file, `OUT_DIR/mf-embedded/<name>/workflow.rs`, containing its normalized
structural plan and preparation entry points. The helper does not load node crates, execute node
constructors, or start a nested Cargo build. Unknown kinds, invalid node configuration, incompatible
ports, and node execution contracts are validated during preparation, not by Rust type checking.

The helper emits `cargo:rerun-if-changed` for definitions. Retain tracking for other inputs when
calling it from an existing build script. Keep workflow JSON in the host package because consumers
compile its build script. Node source/feature changes are ordinary normal-dependency rebuilds;
preparation uses their current metadata without a separate build-time node feature graph.

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

Streaming definitions expose `prepare_stream`:

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

Every module exposes `plan() -> Result<CompiledWorkflow, PlanError>`. It reads embedded program data
and supports existing planning, description, and observation APIs without loading nodes.

`manifest(&registry) -> Result<WorkflowManifest, PlanError>` derives the current configured interface
from the host registry. Inspection prepares separate nodes without executing them. There is no
frozen build-time schema or comparison between build and target node features. Node metadata must
obey the existing preparation contracts and remain stable for the same configuration.

Start task observation from the structural plan before fallible preparation:

```rust
let observation = answer::plan()?.start_observation(
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
