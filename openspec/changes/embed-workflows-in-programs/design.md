# Design

## Context

The current pipeline has two relevant phases. `plan_definition` normalizes structure without linked
providers. The generated `build.rs` links providers and calls `generate_runner_execution_plans` to
validate configuration and contracts and emit layouts, typed preparation, and a manifest.

The source generator assumes `OUT_DIR/flow-plans.rs`. Typed constructors translate workflow aliases
to standalone project dependencies named `node_<index>`. Neither assumption holds for a host with
multiple embedded workflows and its own Cargo aliases.

A procedural macro runs in a separate host process and cannot discover the consuming program's
inventory registrations. The selected design therefore performs provider validation in `build.rs`
and uses a declarative macro only to include generated artifacts.

## Goals and Non-Goals

Goals are build-time provider validation, reusable compiled layouts, ordinary Rust execution APIs,
oneshot and streaming support, and multiple workflows per host. Existing Loop, Iteration, workflow
input, context inspection, and observation semantics must carry through the shared generator and runtime.

The initial change does not add dependency installation, nested Cargo builds, CLI dispatch, automatic
stdio ownership, OTLP setup, or a multi-workflow executable manifest format. It does not expand the
existing supported platforms or cross-compilation guarantees.

## Decisions

### 1. Keep the public API in mf-compiler

Add an `embedding` module with build helpers exported through the existing crate entry point under
`codegen`. Export the declarative inclusion macro independently of that feature. Keep shared
generation APIs at the current module boundary and retain existing visibility decisions.

The intended usage shape is:

```rust
// build.rs
use core_nodes as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let request = mf_compiler::EmbedRequest::new("answer", "workflows/answer.json")
        .with_provider("core", "core_nodes");
    mf_compiler::embed_workflows(&[request], &registry)?;
    Ok(())
}
```

```rust
// src/lib.rs
mf_compiler::embed_workflow!(pub mod answer);

pub fn run() -> Result<mf_runtime::FlowOutputs, Box<dyn std::error::Error>> {
    let registry = mf_runtime::NodeRegistry::from_inventory()?;
    let flow = answer::prepare_workflow(&registry, None)?;
    Ok(mf_runtime::FlowRuntime::default().execute(&flow)?)
}
```

The build helper accepts all workflows as one batch so name validation happens before any writes.
Request paths resolve relative to `CARGO_MANIFEST_DIR`; absolute paths are also accepted. Names must be portable
Rust module identifiers, excluding raw identifiers, keywords, and path separators. Duplicate names
in the batch fail with an attributable error before overwriting an earlier workflow's artifacts.

### 2. Let Cargo own provider selection

The host declares providers in both normal and build dependencies and links them into `build.rs`.
The request maps workflow dependency aliases to the host's Rust crate names. Generated module code
anchors selected normal dependencies so inventory registration is retained even for dynamic nodes.
The caller passes its registry explicitly; no global prepared-node cache is introduced.

Workflow dependency descriptors retain their existing syntax but do not install packages or override
the host Cargo manifest. The embedding contract documents that package versions, sources, and
features are selected by the host. Alias coverage, invalid Rust crate names, unknown providers, and
ambiguous typed constructor selection fail during the build. The host remains responsible for matching
normal and build provider declarations; relevant runtime contract drift is checked before execution.

Generated support crate paths must also work with renamed dependencies. Keep standalone defaults,
offer explicit embedding path configuration where needed, and use `$crate` for the macro's compiler
reference. Do not patch generated Rust through textual substitutions.

### 3. Share the generator and isolate output files

Add focused generation parameters for artifact paths and embedding entry points. Standalone
generation keeps its current default paths, `node_<index>` aliases, and exported API.

The embedding helper reads the definition, calls the existing structural planner, and obtains layouts,
manifest data, and preparation from the existing linked-provider validation path. It emits a
minimal embedding module with preparation and manifest access rather than a runner `main` or
unused execution convenience functions.

Embedding exposes the existing general-purpose/custom-runner contract, including intermediate
outputs in `ExecutionContext`. Use `generate_execution_plans` semantics, which currently select the
`CustomRunner` typed fast-path fallback, rather than the standard runner's output-eliding profile.
Typed providers remain supported through ordinary dynamic execution. Do not change that fallback
or parameterize standalone typed constructor aliases solely for this initial embedding feature.

Each request owns `OUT_DIR/mf-embedded/<name>/`. The macro includes that workflow's module; all
nested layout and byte includes resolve within the same directory. Validate the complete workflow
before publishing its module, using existing atomic/write-if-changed helpers. No files outside the
request's output directory are rewritten.

The generated oneshot module returns `Flow`; its streaming counterpart returns `PreparedStream`.
Frozen layouts are reusable, but each preparation constructs independent provider instances. Host
execution uses `FlowRuntime`, `ExecutionContext`, `RuntimeOptions`, and `StreamOptions` directly.

### 4. Make preparation enforce the embedded contract

Expose `manifest()` with the existing `WorkflowManifest` format. Preparation must apply the same
applicable metadata and startup schema checks as existing generated execution before returning a usable
flow or stream. Construction, manifest, and build I/O failures retain typed Snafu sources and identify
the embedding name, input path, or affected node as appropriate.

Manifests are module data, not additional `.mf_manifest` or `__mf_manifest` executable sections.
The current binary inspection contract assumes one standalone workflow. Multiple embedded
workflows therefore do not automatically make an arbitrary host compatible with `mf run --tui`.
Hosts can use each manifest with the existing observation APIs and manage their own exporters.

### 5. Respect incremental builds and host resource ownership

Emit Cargo change tracking for every definition and any other helper-read input, even when the host
already has a build script. Provider source and feature changes remain ordinary Cargo dependencies
of that build script. Do not create an independent provider cache or reuse unvalidated stale artifacts.

Preparation must not execute node business operations. Credential and external resource acquisition
retain their current preparation/execution boundaries. Execution performs no definition file reads,
Cargo subprocesses, or graph repartitioning. Hosts explicitly supply inputs, stdin handles, streaming
resources, cancellation, snapshots, and observation; generated embedding code does not claim global
stdio or initialize telemetry.

## Risks and Validation

- Host and build dependency features can differ under Cargo's resolver. Test compatible configurations
  and incompatible startup/typed metadata, and document the provider contract.
- Typed fast paths currently assume a standard runner and ordinal crate names. Preserve the existing
  custom-runner fallback, and cover renamed typed providers through real dynamic host execution.
- Per-workflow files and symbols can collide. Compile two workflows with overlapping node IDs,
  including mixed modes, and check independent instance state.
- Added JSON fixtures may be filtered out of Nix sources. Keep them in included example paths or
  explicitly update the source fileset and Cargo package inclusion.
- Shared generator changes can affect standalone builds. Retain their existing compilation, manifest,
  typed execution, streaming, and warning checks.

No persisted definition or manifest migration is required. Implement the generation parameters first,
then build-time embedding, then macro inclusion and preparation contracts, and finally acceptance
coverage and documentation.
