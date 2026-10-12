# Embedding workflows in Rust programs

Use `mf-compiler`'s build helper and `embed_workflow!` macro to add workflows to an existing Rust
application or library. The host build validates graph structure, node configuration, inferred ports,
and execution contracts. Generated modules bind provider instances to compiled layouts; execution
does not read definitions, invoke Cargo, or repartition graphs.

## Cargo setup

Enable `codegen` in build dependencies and disable the compiler's default features in normal
dependencies. Declare each provider in both sections with compatible sources, versions, and features:

```toml
[dependencies]
mf-compiler = { version = "=0.1.0", default-features = false }
mf-runtime = "=0.1.0"
core_nodes = { package = "mfn-core", version = "=0.1.0" }

[build-dependencies]
mf-compiler = { version = "=0.1.0", default-features = false, features = ["codegen"] }
mf-runtime = "=0.1.0"
core_nodes = { package = "mfn-core", version = "=0.1.0" }
```

The host's Cargo manifest and Cargo.lock select packages and features. Dependency descriptors inside
workflow JSON retain their existing format but do not install packages, resolve local paths, or
override the host manifest. Binding `core` to `core_nodes` associates a workflow dependency alias with
its Rust crate name in the host. Bind every declared workflow dependency exactly once.

Keep build and normal provider declarations compatible. Cargo can resolve their features separately.
Providers must obey the existing contract that metadata is stable for the same configuration across
build and execution environments. Preparation checks the compiled startup interface before execution.

## Build-time validation

Add a build script:

```rust
// build.rs
use core_nodes as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let answer = mf_compiler::EmbedRequest::new("answer", "workflows/answer.json")
        .with_provider("core", "core_nodes");
    mf_compiler::embed_workflows(&[answer], &registry)?;
    Ok(())
}
```

Import providers in the build script so their inventory registrations are linked. The generated target
module anchors the bound provider crates automatically. The build helper does not execute node
business operations and does not start a nested Cargo build.

Definition paths are relative to `CARGO_MANIFEST_DIR`; absolute paths are accepted. Submit all
workflows in one batch. Names must be ASCII Rust module identifiers, excluding keywords, raw
identifiers, and `_`; duplicate names fail before files are replaced. Each workflow owns
`OUT_DIR/mf-embedded/<name>/`, containing module source, execution layouts, and a manifest.

The helper emits `cargo:rerun-if-changed` for definitions. Provider source and feature changes use
Cargo's ordinary build-script dependency tracking. Existing build scripts can call the helper alongside
their other generators; retain tracking for their other inputs. Keep the workflow JSON available when
packaging the host crate, because consumers compile its build script.

## Oneshot calls

Include the generated module and prepare a flow:

```rust
mf_compiler::embed_workflow!(pub mod answer);

fn run() -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let flow = answer::prepare_workflow(&registry, None)?;
    Ok(mf_runtime::FlowRuntime::default().execute(&flow)?)
}
```

A prepared `Flow` can be executed repeatedly. Each preparation constructs independent provider
instances; providers can retain state across executions of the same instance. The host controls
`RuntimeOptions` and supplies startup inputs with `execute_with_inputs`:

```rust
let arguments = mf_runtime::WorkflowArguments::from_json(
    br#"{"request.text":"hello"}"#,
)?;
let outputs = runtime.execute_with_inputs(&flow, arguments)?;
```

This example assumes a workflow exposing `request.text`. Values use the existing workflow input
contract. For intermediate outputs, host-supplied stdin, snapshots, or observation, execute through
your own `ExecutionContext`:

```rust
let mut context = mf_runtime::ExecutionContext::default();
context.set_workflow_arguments(arguments);
let outputs = runtime.execute_in_context(&flow, &mut context)?;
let intermediate = context.output("request.result")?;
```

Embedding follows the general-purpose/custom-runner contract and preserves intermediate context
outputs. Typed providers execute through the existing dynamic path; standard-runner typed fast paths
that elide intermediate outputs are not enabled for embedding. Loop and Iteration keep their existing
scope, limits, and error behavior.

## Streaming calls

A workflow declaring streaming execution exposes `prepare_stream` instead of `prepare_workflow`:

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

Here `consume` is the host's output handler. Drain deliveries before calling `join`. Starting a stream
consumes its `PreparedStream`; prepare again for another instance. Provide startup arguments, stdin,
observation, and snapshots through `StreamOptions`. For a workflow requiring stdin, supply a
`TextInput` explicitly, for example `TextInput::new(file)`. The module does not claim global stdio or
initialize exporters. Use the instance's cancellation handle to stop an ongoing stream.

## Manifests and observation

Both module kinds expose `manifest() -> Result<WorkflowManifest, WorkflowManifestError>`. The
manifest uses the existing graph and startup interface format and retains the compiled workflow
identity. Hosts can use its description with their existing observer. For a oneshot workflow:

```rust
let manifest = answer::manifest()?;
let observation = observer.start_with_description(
    manifest.description,
    mf_telemetry::identity::RunId::new(),
)?;
let outputs = mf_runtime::ExecutionContext::run(Some(observation), |context| {
    let flow = answer::prepare_workflow(&registry, context.observation_mut())?;
    Ok::<_, Box<dyn std::error::Error>>(runtime.execute_in_context(&flow, context)?)
})?;
```

The host supplies `observer` and declares any telemetry dependencies it uses. Embedded manifests are
module data rather than standalone executable manifest sections. A host with several workflows is
not automatically a runner accepted by `mf run --tui`; the host owns invocation and observation setup.

## Repository example

The [embedded-workflows example](../examples/embedded-workflows) uses local support crates and prints
`{"answer":42}`. From the repository root:

```sh
CARGO_TARGET_DIR="$PWD/target/embedded-example" nix develop --command \
  cargo run --manifest-path examples/embedded-workflows/Cargo.toml --offline
```

After building, the executable runs without its workflow JSON, generated sources, or build tools.
