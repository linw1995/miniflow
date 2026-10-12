# Design

## Context

The compiler separates structural planning from linked-node preparation. `plan_definition` normalizes
structure without a registry. `instantiate_compiled` and `instantiate_stream` construct nodes, infer
ports, validate contracts, and prepare execution layouts with a registry.

Embedding generates source during the host build. Node crates are linked only into the final program,
where preparation validates their actual configuration and metadata. Standalone `mf compile`
continues validating nodes in its generated build script.

## Decisions

### Build the structural plan

Keep `EmbedRequest { name, definition }` and codegen-gated `embed_workflows(&[request])` at the existing
compiler entry point. Read definitions relative to `CARGO_MANIFEST_DIR`, emit Cargo input tracking,
and plan structure without a registry or node bindings.

Validate portable names and the complete batch before replacing modules. Keep typed Snafu sources,
atomic/write-if-changed behavior, and output directory symlink checks. Each workflow emits only
`OUT_DIR/mf-embedded/<name>/workflow.rs`.

### Reuse ordinary preparation

Generate a module containing a serialized normalized plan and calls to existing compiler APIs.
Parsing reads embedded program data, never definition files. The macro uses `$crate` and a runtime
re-export so host dependency renames work without target-side codegen.

Task modules expose `plan`, `manifest`, and `prepare_workflow`; streaming modules expose `plan`,
`manifest`, and `prepare_stream`. Preparation returns `Flow` or `PreparedStream` through existing
`PlanError` sources. Each preparation constructs independent nodes; execution reuses prepared
layouts with `FlowRuntime` and retains intermediate outputs.

Share attributed task preparation failure reporting between `execute_compiled` and embedding.
Keep streaming preparation and observation on existing APIs. Add no embedding execution engine,
generated layout profile, or prepared instance cache.

### Let the host link nodes

Hosts declare node crates only as normal dependencies and link registrations with ordinary imports
such as `use core_nodes as _;`. The build script needs only `mf-compiler` with `codegen`. Workflow
dependency descriptors retain their format but do not resolve or override Cargo packages. Remove
`with_provider` and the duplicate host/build node dependency graph.

Unknown kinds, invalid node configuration, incompatible ports, and execution metadata fail during
preparation before execution. Malformed definitions, duplicate IDs, missing endpoints, and cycles
fail during source generation. Rust compilation checks generated code, not dynamic node names or
JSON configuration.

### Describe the current interface

`plan()` exposes `CompiledWorkflow` and its observation setup APIs, allowing observation before
fallible node preparation. `manifest(&registry)` uses existing description/interface APIs to return
a validated existing-format manifest from current host metadata. Manifest inspection prepares
separate nodes without executing them.

Freeze no build-time startup schema. Actual node metadata governs interfaces and layouts. Emit no
standalone executable manifest section or multi-workflow `mf run --tui` runner contract.

### Store fixed host inputs as fixtures

Keep Cargo, Rust, and JSON inputs under `tests/fixtures/embedding-host`. Copy only inputs into a
temporary host, rewrite dependency paths, and apply scenario mutations there. Keep small mutations
in the test; do not duplicate fixture projects. Include fixtures in Nix and Cargo sources.

## Validation and Review

Compile a host with renamed dependencies, codegen only in build dependencies, and node crates only
in normal dependencies. Cover task/stream preparation, state reuse/isolation, typed intermediate
outputs, Loop/Iteration, resources, and observed preparation failure. Structural errors must preserve
the prior binary; node contract errors must compile and fail during preparation.

Check warm definition edits, target-node metadata edits, execution after removing source/build
files, and the documented example. Retain standalone regressions. Run negative ablations for input
tracking, ordinary node linking, observation reporting, and source independence. Keep reports under
ignored `target/`; validate, commit, and archive only after review.

## Review Decisions

Keep the runtime re-export required for renamed dependencies and the shared observed preparation
entry point used by generated and in-memory execution. Negative ablations confirm Cargo tracking,
normal node linking, preparation failure reporting, and source-independent execution are necessary.
Remove the old generation profile, provider map, extra artifacts, and frozen schema comparison.

The new preparation boundary requires kind/config/port acceptance cases even though lower-level
validators have their own tests. Avoid a duplicate helper success test. Keep Cargo.toml.in fixtures
so Cargo packages include the host inputs instead of excluding a nested package. Detailed reports
and timings remain under ignored `target/`.
