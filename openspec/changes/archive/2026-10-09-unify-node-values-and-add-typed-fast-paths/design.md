# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. `NodeInputs` and `NodeOutputs` currently expose the same port
declaration shape but different conversion methods. `TypedTaskNode` associates one struct with each role, and
`execute_typed_task` always decodes an input map and encodes an output map.

`NodeRegistration` selects a factory by kind; `PreparedNode` erases the concrete task into `Box<dyn TaskNode>`.
Generated preparation in `mf-compiler/src/plan.rs` binds those executors to frozen layouts and delegates execution to
`FlowRuntime`. The generated Cargo build has the selected plugins available, whereas the installed CLI does not.
Existing execution domains already provide the scheduling boundary for a generated serial segment.

## Goals / Non-Goals

**Goals:**

- Give providers one authoritative named-struct contract while preserving directional error attribution at dynamic
  boundaries.
- Eliminate intermediate map construction and encode/decode round trips where the generated program can prove equivalent
  typed behavior.
- Reuse the common scheduler, node lifecycle, and transactional publication rules.
- Make every omitted check and every moved value justify itself through explicit compile-time evidence.

**Non-Goals:**

- Rust type reflection from JSON descriptors, persisted `TypeId` values, implicit `From` conversions, or unsafe casts.
- A second graph executor or automatic public exposure of existing implementation types.
- Deferred context materialization, typed shared frames, arbitrary nested record schemas, or new Serde blanket
  implementations in the first version.

## Decisions

### 1. Make NodeValue the canonical named-port contract

Add `NodeValue` with `ports()`, `from_values(...)`, and `into_values(...)`, and a corresponding derive with
`#[value(rename = "...")]` and an explicit runtime path. Introduce `NodeValues` as the common map name and retain
`Inputs` and `Outputs` as aliases. The conversion methods reuse the existing source-bearing decode and encode errors;
their direction is useful diagnostic context even though the data type is unified.

The derive parses fields once, generates the canonical implementation, and generates explicit compatibility
implementations of the existing directional traits that delegate to it. Avoid reciprocal blanket implementations, which
can overlap with existing manual implementations. Keep the existing `TypedTaskNode` associated-type bounds during this
compatibility period: a newly derived value satisfies both automatically, and old one-direction providers remain valid.
Removing the old contracts or tightening those bounds is a later breaking migration, not part of this change.

Preserve all existing owned scalar, list, map, optional-port, alias, generic, rename, and runtime-path semantics. The
contract describes an entire port bag, not a nested JSON record. User-owned input and output structs work with the
derive; automatic nested struct/enum codecs remain a separate extension. Existing custom field codecs continue to work,
but do not automatically qualify for static transfer.

Alternative: rename the two macros without a canonical implementation. Rejected because separate declarations can still
drift. Alternative: require both codecs immediately on every legacy provider. Rejected because it breaks input-only and
output-only implementations unnecessarily.

### 2. Collect typed generation information before executor erasure

Add an optional generation descriptor alongside the existing registration contract. Providers without one retain their
dynamic behavior. The descriptor supplies structured references to a provider-owned exported construction/invocation
shim, concrete input and output contracts, field access/construction helpers, and the codec/validation evidence needed
for planning. It must agree with the ordinary factory's configured metadata.

Collect descriptors in the generated Cargo build with linked providers, and emit a preparation macro alongside the
frozen layout items in `flow-plans.rs`. Standard runners expand it; custom runners retain dynamic preparation. The CLI keeps generating the dependency project without inspecting plugin source or naming
plugin internals. Provider crate references resolve through the Flow's dependency aliases; host-only generation metadata
must not introduce a compiler dependency into runtime execution or provider business logic. A provider can expose one
dedicated shim while keeping its executor and fields private. The derive supplies tuple construction/decomposition through `TypedNodeValue`, so private fields stay private.
The first version uses a closed, runtime-owned Rust representation enum for certified scalar and collection types;
custom Rust types remain opaque to generation. Exhaustive metadata literals must include the new optional field.

Use structured, validated paths and identifiers, not arbitrary code strings or whole-crate visibility changes. For
candidate matching, use provider-declared canonical type references with package identity. Generated Rust assignments
are the final type-equality proof. Aliases that cannot be recognized conservatively fall back; matching JSON
`ValueType`, short names, or serialized host `TypeId` is never sufficient. Missing optional metadata means fallback;
malformed supplied metadata or a failing advertised Rust assignment means a build error, not a hidden retry that masks a
plugin defect.

Alternative: downcast prepared trait objects during execution. Rejected because it does not restore static field wiring
and introduces runtime type plumbing. Alternative: teach the CLI every node kind. Rejected because plugins are selected
independently by each Flow.

### 3. Plan complete serial segments conservatively

The first lowering targets standard generated top-level oneshot task domains. In-memory execution, custom-runner
artifact generation, streaming, and nested bodies retain the dynamic path. A segment is a serial chain inside one
existing execution domain; its entry and exit use ordinary dynamic contracts. Generate maximal eligible chains with at
least one direct internal data connection, without changing domain partitioning.

Every internal transfer requires static port names, a required source and target field, identical Rust field types,
compatible resolved descriptors, and certified equivalent field validation. Optional internal fields, opaque custom
codecs, unproven output refinements, and instance-dependent port names terminate the segment. Optional unrelated fields
still retain ordinary validation and omission behavior. No data conversion between different Rust types is inferred.

Compute uses from data and control edges, workflow selections, declared context references, and runtime payload
observation requirements. A value moves only if its typed consumer is its sole remaining observer. A selected output or
context-readable value cannot disappear after a move. Exclude segments with contextual reads or retained intermediate
outputs that cannot be materialized before ownership transfer; do not add a hidden `Clone` bound. Fan-out and joins
remain dynamic boundaries.

Provider generation descriptors must promise that omitted dynamic publication is not observable through undeclared
context reads. Existing providers lacking that promise remain dynamic. Custom runners that expose the final context
retain full dynamic publication. When input/output snapshots are requested at runtime, dispatch the already-prepared
dynamic implementation for the affected domain; normal node lifecycle telemetry can remain enabled on the typed path. No
runtime graph planning is performed.

Alternative: optimize any edge with equal Rust names. Rejected because it overlooks codec constraints and value
observers. Alternative: implement lazy dynamic views immediately. Deferred because preserving a view after moving an
owned value requires an explicit shared representation and a larger lifetime contract.

### 4. Bind generated segments to the common domain executor

Extend the prepared executable plan's domain binding to accept a generated typed serial body and its dynamic fallback.
Keep `FlowRuntime` responsible for readiness, worker limits, private invocation contexts, commits, cancellation, and
ordered scope effects. Bind only validated layouts at launch; do not reconstruct the graph.

Both invocation strategies bind to the same constructed provider instance. The exported shim constructs it once and
exposes a typed handle together with its ordinary dynamic adapter, using shared immutable task ownership where needed.
Selecting snapshot fallback must not call another factory, duplicate resource acquisition, or reset stateful task
behavior. Metadata checks reuse the validated preparation and descriptor declarations rather than constructing another
instance solely for generation or manifest inspection.

Factor the existing per-node lifecycle so generated invocations and ordinary task adapters share dependency
availability, missing-before-skip precedence, observation phases, error attribution, and staged effects. Generated code
constructs concrete inputs and directly invokes provider shims inside that lifecycle. The result retains the existing
typed envelope for explicit skips and loop summaries. Each invocation creates fresh typed locals; no mutable frame is
shared across parallel domains or workflow instances.

Keep presence state separate from payload ownership. Validate dependency availability before input assembly. A skip
prevents construction and invocation; a missing required dependency is still an error. If an explicit skipped output is
consumed later, mark the consumer skipped without attempting to move a nonexistent value. Preserve checks for invalid
produced/skip combinations.

### 5. Replace validation only with equivalent typed validation

Validate the entire producer result before any field becomes available to a successor. Built-in typed validators can
discharge structurally guaranteed checks and check remaining value constraints without constructing JSON: finite floats,
recursive descendants, requiredness, skips, and resolved refinements must still be enforced. Validate unused fields too.
Retain the same producer/input attribution, nested escaped pointers, source-bearing error categories, and failure phase
as the dynamic path.

Matching Rust types is not permission to bypass custom decoding validation. A custom codec without a certified typed
validator and compatible encode/decode semantics forces dynamic fallback. `ValueRef` can transfer without copying its
payload, but broad-to-refined contracts still require the existing shared-value validation. Unsupported refinements
terminate a segment rather than weaken publication checks.

After successful typed validation, internal values stay in typed locals. Encode values needed at a dynamic
segment/domain exit or workflow-output boundary before committing that boundary. Encoding failure prevents partial
publication. If output validation or boundary encoding could fail, it must complete before any downstream business call
that would not run on the dynamic path. Ordinary boundary conversions continue preserving shared descendants and numeric
representation.

### 6. Prove behavior and the absence of conversion separately

Use external provider fixtures with renamed dependencies and private executor types. Compare generated typed execution
against the in-memory dynamic reference for successful values, business errors, omitted/skipped/missing outputs,
non-finite nested floats, refinements, and worker limits. Exercise runtime snapshot fallback and custom-runner
compatibility.

For eligible chains, assert that generated source contains typed construction/field transfer and no internal
encode/decode or intermediate map operations. Count generated boundary conversion calls and check owned allocation identity with concrete string/list fields to
demonstrate that boundary conversion remains but internal round trips disappear. Test unproven
custom validation as a fallback case. Keep timing/allocation comparisons and ablation reports under ignored `target/`;
functional tests should not depend on noisy timing thresholds.

## Risks / Trade-offs

- Restricted first-version coverage -> Report deterministic fallback reasons in build inspection data; expand from
  proven cases rather than changing semantics.
- Output moves can invalidate observable context -> Include every known observer in use analysis, require the provider
  context-read contract, and use dynamic execution for payload snapshots and custom-runner context inspection.
- A float or custom codec can fail despite Rust type equality -> Validate before exposing results and reject unproven
  codec equivalence from fast-path eligibility.
- Host/target paths and crate aliases can disagree -> Resolve through selected package identities and compile the
  generated references in the target runner; preserve ordinary metadata agreement checks.
- Generated monomorphized code increases compile time and binary size -> Generate only eligible segments, share
  lifecycle helpers, and measure size/build-time alongside runtime allocation changes.
- Compatibility adapters may duplicate trait implementations -> Generate explicit bridges only for the new derive and
  cover old derives, manual contracts, generics, and duplicate-derive diagnostics with compile cases.

## Migration Plan

1. Add the canonical contract and explicit directional bridges; migrate typed built-ins without altering dynamic
   provider behavior.
2. Add optional provider descriptors and external fixtures; prove metadata agreement and alias handling before changing
   execution.
3. Add common lifecycle hooks and generated domain binding, retaining the existing dynamic executor as fallback.
4. Enable typed lowering only for proven standard oneshot segments and document scope and fallback behavior.
5. Run semantic differential tests, conversion elimination checks, pinned repository validation, and OpenSpec
   validation. Keep implementation tasks unchecked until those steps actually complete.

Rollback consists of disabling typed lowering while keeping the unified contract and dynamic adapters. No saved Flow
definitions or manifest payloads require conversion. Regenerated build artifacts must reflect the current descriptor and
generator inputs; cached source must not preserve stale eligibility decisions.
