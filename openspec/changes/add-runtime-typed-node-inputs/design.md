# Design

## Context

See `proposal.md` for motivation. `Inputs` is a map of port names to shared `ValueRef` values. `PreparedNode` stores independently supplied `NodeMetadata` and a task, event, or stream executor. Tasks are stored as `Box<dyn TaskNode>` and their dynamic execution method is used by synchronous flows and stream message domains.

The compiler validates metadata and constructs executable plans. Runtime task invocation resolves dependencies, handles skips, checks bound inputs, records execution, invokes the task, and validates outputs before publication. Generated runners freeze startup declarations and compare them with runtime preparation. The typed interface must fit these boundaries without introducing compiler dependencies into the runtime.

## Goals / Non-Goals

**Goals:**

- Make one input struct the authority for both port declarations and decoded business inputs.
- Own schema generation helpers, decoding, adaptation, and preparation in the runtime API.
- Keep type erasure at preparation so scheduling remains independent of concrete provider types.
- Support external providers, generated runners, shared JSON values, and existing typed error chains.

**Non-Goals:**

- Change the existing dynamic execution traits or remove configuration-dependent port declarations.
- Add typed event/stream traits, typed outputs, or structural record descriptors for nested structs.
- Add input defaults, JSON coercions, Serde flattening, or arbitrary deserialization hooks.
- Change skip precedence, execution observation phases, worker ownership, or graph compilation.

## Decisions

### 1. Add an execution-only typed task contract

Add public `NodeInputs` and `TypedTaskNode` contracts through the existing runtime module entry point:

```rust
pub trait NodeInputs: Sized {
    fn ports() -> Vec<PortSpec>;

    fn from_inputs(inputs: Inputs) -> Result<Self, InputDecodeError>;
}

pub trait TypedTaskNode: Send + Sync {
    type Input: NodeInputs;

    fn execute(
        &self,
        input: Self::Input,
        ctx: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError>;
}
```

A private runtime adapter implements `TaskNode` for a typed provider. It consumes the dynamic input map, decodes `N::Input`, and calls the typed execution method. The adapter stores the provider, not per-invocation inputs, and retains the provider's existing `Send + Sync` boundary. There is no scheduler downcast or runtime reflection.

Adding an associated input type directly to the existing `TaskNode` would disrupt its heterogeneous trait-object storage. Keeping conversion in provider methods would preserve the duplication this change is intended to remove. A separate typed contract provides an additive migration path.

### 2. Generate declarations during typed preparation

Add `PreparedNode::typed_task(node, metadata) -> Result<PreparedNode, NodeBuildError>`. Reuse `NodeMetadata` for caller-supplied outputs, derivations, references, and stdin requirements. Require `metadata.ports.inputs` to be empty; a nonempty input declaration returns a typed construction error even when it appears to match the generated ports.

The constructor fills inputs from `N::Input::ports()` and wraps the executor. It never executes the provider or decodes invocation values. Existing compiler metadata validation continues to check names, descriptor depth, derivations, references, and resource contracts. The constructor does not perform graph validation or initialize invocation state.

This prevents accidental schema overrides while retaining current metadata ergonomics. Introducing a second metadata structure for every field except inputs would duplicate the existing preparation contract. The fallible constructor makes misuse observable instead of silently overwriting declarations.

### 3. Keep field semantics in runtime codecs

Add runtime field codecs with separate responsibilities for a present value and a possibly missing top-level field. The derive delegates to these codecs, including requiredness, rather than recognizing Rust type names syntactically. Type aliases therefore retain the same behavior.

Support these owned types initially:

| Rust type | Port descriptor | Required |
| --- | --- | --- |
| `bool` | `Boolean` | Yes |
| `i64` | `Int64` | Yes |
| `f64` | `Float64` | Yes |
| `String` | `String` | Yes |
| `ValueRef` | `Any` | Yes |
| `Vec<T>` | `List(T)` | Yes |
| `BTreeMap<String, T>` | `Map(T)` | Yes |
| `Option<T>` | Descriptor of `T` | No |

Collection element/value types must implement the present-value codec. `Option<T>` only implements the top-level field codec, so nullable collection elements and nested optional fields are not implicitly accepted. Implement supported field codecs explicitly rather than using overlapping blanket implementations for required and optional fields. Public manual implementations carry the same declaration/decoding consistency obligation as existing plugin port declarations.

Missing optional fields decode to `None`; supplied values decode to `Some(T)`. Supplied null is checked against `T`: `Option<String>` rejects null and `Option<ValueRef>` produces `Some(ValueRef::null())`. Required fields reject omission. Unknown input keys are rejected. No defaults or coercions are introduced, and decoding honors the existing descriptor depth limit.

Decode directly from shared values. Move a `ValueRef` field's handle out of the map. For collections, decode children from shared handles; owned strings and typed collection storage may allocate. Do not serialize an entire input map into text or materialize an intermediate `serde_json::Value` tree. Runtime codecs reuse the existing type validation rules, including strict floating-point representation and nested JSON Pointer diagnostics.

### 4. Keep the derive package independent of the runtime implementation

Create the `mf-runtime-derive` proc-macro crate and re-export its derive through `mf-runtime`. The macro crate depends on code-generation libraries and emits references to runtime contracts; it does not depend on `mf-runtime`, avoiding a dependency cycle. Use existing workspace code-generation dependencies and pinned toolchain.

The first derive supports named-field structs, including generic fields with generated codec bounds and empty named-field structs. Reject enums, tuple/unit structs, unsupported field types, and borrowed fields with actionable compiler diagnostics. Field names map to port names, with raw identifiers normalized. Support `#[input(rename = "port-name")]` and reject empty or duplicate resulting names. Do not interpret Serde attributes as input declarations.

Default generated paths target `::mf_runtime`. Support an explicit struct-level runtime path override, such as `#[input(runtime = "::runtime_alias")]`, for dependency aliases and an internal crate path when needed. The override is compiled as a Rust path rather than pasted as unchecked tokens. Expansion delegates decoding and diagnostics to runtime helpers instead of duplicating conversion logic in generated provider code.

### 5. Preserve execution ordering and typed errors

Dependency resolution, missing-output errors, scheduler skips, and existing input type checks run before the adapter. A skipped node never decodes inputs or executes typed business logic. The adapter converts decode errors into a source-bearing runtime execution error using Snafu; workflow errors retain node identity through the existing invocation wrapper.

Define shared decode errors in the runtime for missing fields, unknown fields, and invalid present values. Preserve `TypeMismatch` and its JSON Pointer as a typed source. Attach port context without stringifying the source. Node business errors remain in provider crates and use the existing plugin error boundary.

Adapter failures use the current execution failure phase because the adapter runs inside task invocation. Existing bound-value checks continue to report their current validation phase. Avoid scheduler restructuring or promises that all decode failures happen before other nodes execute. Startup argument validation remains authoritative and continues to validate the complete argument object before dispatch.

### 6. Use the existing prepared schema everywhere

Compiler connection checks, startup argument validation, manifests, description, and generated execution consume the generated `NodeMetadata` without typed-node special cases. Descriptor inference remains independent of invocation values and process state.

Migrate `builtin.identity` to a `ValueRef` input struct and keep its forwarding derivation. Verify that the derived descriptor remains `Any` and execution forwards the shared payload. Use an external fixture with refined scalar, collection, optional, and renamed inputs to exercise more than identity's broad contract. Legacy and configuration-dependent providers remain on dynamic interfaces.

Typed stream and event adapters can reuse the input contracts later. Their mutable state, input-only event conversion, and timer/close semantics require separate interface work and are not part of this implementation.

## Risks / Trade-offs

- Schema and decoding drift in custom implementations: document the codec invariant and verify every provided codec against its advertised descriptor and requiredness.
- Optional null behavior differing from Serde: document and test omission, `Some(null)` for shared JSON, and rejection of null for typed scalars.
- Added proc-macro dependency and packaging requirements: verify workspace builds and an external provider importing only `mf-runtime`, including a renamed dependency.
- Duplicate validation of supplied values: preserve current runtime checks for safety and use shared validation primitives in codecs; optimize only with evidence from subsequent profiling.
- Shared-value regressions hidden by equal JSON results: assert shared payload identity as well as value equality in identity and nested collection tests.

## Migration Plan

1. Add runtime contracts, strict codecs, and source-bearing errors with focused tests.
2. Add the derive package, re-export, and compile-pass/fail coverage for its supported shape and attributes.
3. Add typed preparation and the task adapter, preserving existing dynamic execution APIs.
4. Migrate identity and add external-provider integration coverage and documentation.
5. Run the required repository and OpenSpec checks before implementation is submitted.

The current task produces planning artifacts only. Implementation tasks remain unchecked. The additive implementation can be rolled back by restoring identity's dynamic input handling and removing the new typed API and derive package; workflow definitions require no migration.
