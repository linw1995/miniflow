## Context

See [proposal.md](proposal.md) for motivation and scope. Today `ExecutionInputs::resolve_node` collects every skipped dependency as a consumer skip cause, while compiler validation rejects multiple edges to an input. Type inference and typed-plan lookup also assume one binding per input. Generated tasks and stream operators share input-resolution machinery, but stream skip observations additionally inspect raw dependencies.

The earlier test-selection proposal in another worktree proposed `builtin.merge`. This change replaces that architectural direction with explicit core bindings. It does not modify that worktree or implement the test-selection workflow.

## Goals / Non-Goals

**Goals:** Make alternative-source resolution a graph-level input contract, preserve ordinary node interfaces and activation, and share validation and resolution across supported execution paths.

**Non-Goals:** General activation expressions, scheduler plugins, synthetic provider nodes, cross-domain correlation, fallback on errors, stream union, or a new type-union system. No changes to module visibility are required by this design.

## Decisions

### 1. Declare alternatives separately from ordinary edges

Add default-empty `exclusive_inputs` to workflow, loop body, and iteration body definitions:

```json
{
  "exclusive_inputs": [
    {
      "to_node": "diff",
      "to_input": "request",
      "sources": [
        { "from_node": "local", "from_output": "value" },
        { "from_node": "ci", "from_output": "value" }
      ]
    }
  ]
}
```

An entry binds one existing port and owns all its candidates. At least two distinct candidates are required. Reject another group or an ordinary edge targeting that port. Keep ordinary duplicate-edge rejection. Use explicit source pairs, not dotted-string parsing. Empty arrays are omitted on serialization, and unknown fields remain errors.

This keeps grouping explicit without adding policy flags to every edge or teaching providers about workflow branches. The supported definition versions can accept the additive field using the existing optional-field compatibility approach; old readers reject it through strict deserialization. Update Rust struct construction sites and document the low-level API addition.

### 2. Preserve groups in the prepared input plan

Represent single-source bindings and exclusive source groups explicitly in the shared prepared plan, with control dependencies remaining separate mandatory gates. Flatten candidate source references only when a graph algorithm needs predecessor adjacency, never when deciding whether a consumer should skip.

Every candidate participates in topological ordering, cycle and endpoint validation, context ancestry, worker-domain visibility, message-domain analysis, and connected-input accounting. Use the same preparation path for definition-based and programmatic construction. Carry groups through nested body preparation and generated plans; do not introduce hidden nodes with new execution identities.

Ordinary scheduling already resolves predecessor outcomes. Retain that ordering and wait for all candidate producers; the resolver must not block a worker or poll. A missing output after predecessor completion is an error. A skipped stream producer does not invent an output-domain frame, so this feature cannot join independent producers or bridge a stream/collector boundary.

### 3. Resolve groups before applying ordinary activation

For each group, inspect every candidate in the current context. Produce one resolved input value, an all-skipped input outcome, or a dependency error. Count present false and null as produced values. Preserve the selected `ValueRef` and its sensitivity metadata directly.

Inspect dependencies before honoring skip outcomes. Unexpected absence or group ambiguity must not disappear behind another skipped dependency. Preserve existing missing-before-skip and skipped-node-before-input-type-check behavior. Missing and ambiguity diagnostics must be stable under candidate reordering; if both occur, report missing output first, then ambiguity after all references are known to exist.

Combine resolved groups, ordinary inputs, and controls conjunctively. Only all-skipped groups contribute candidate skip causes; an unselected candidate in a successful group does not. Update stream observation paths to consume resolved causes instead of reconstructing causes from every raw skipped source.

The consumer still receives its ordinary input map or typed input structure, never candidate lists or skip markers. A connected optional group with no selected value skips like a connected optional edge. For collecting consumers, that outcome excludes the item without altering buffered values or deadlines. Upstream failure and cancellation remain workflow termination conditions, not selection fallbacks.

### 4. Keep type evidence sound across alternatives

Validate every source against the target using existing compatibility and known-value rules, including candidates expected to skip. Do not infer exclusivity from `if_else` topology; runtime cardinality validation remains authoritative.

For output derivation through a selected input, keep a common source type when all candidate type facts agree. Otherwise use the validated target input type as conservative evidence. Keep an exact-value fact only when all candidates prove the same value; otherwise discard it. This avoids arbitrary map overwrite and does not require union types or a new least-common-type algorithm. Runtime input checks remain required at dynamic boundaries for active consumers.

Initially exclude candidate-to-consumer transfers from ownership-based typed fast paths using a specific fallback reason. The ordinary shared-value path is the semantic reference. Continue optimizing unrelated safe segments; generated execution must retain groups even when optimization falls back.

### 5. Preserve scope and interface boundaries

Apply the same plan and resolver to ordinary Tasks, Event/Stream input delivery, loop rounds, and sequential or parallel iteration items. Support existing virtual scope sources within their legal scope. Reject out-of-scope candidates and differing input message domains during preparation. Never retain a selected source or value between invocations.

Each port group selects independently. For correlated fields such as a request and its provenance, select one structured value rather than implicitly coupling two groups. Isolated node execution receives the already resolved input through its existing arguments and does not run predecessors.

### 6. Expose input provenance through existing observation

Extend workflow descriptions to distinguish candidate groups from mandatory dependencies. Attach selected-source identity to the consumer's input-resolution observation, scoped to its frame/body invocation, without including payloads. Keep dependency errors source-bearing and typed through Snafu; report target port and relevant source identities. Existing snapshot redaction applies to selected values.

Selection is part of consumer preparation, not another user-visible node or span. Update manifest/schema fixtures and readers together so the TUI and other consumers do not interpret alternatives as independent AND inputs. No new dashboard or observation subsystem is needed.

## Alternatives Considered

- **Explicit merge node:** provides a reusable, visible forwarding result but still requires core support for skipped candidates. Rejected here because the selected design puts this behavior on the consuming input.
- **Global OR or node-wide any policy:** cannot preserve independent required inputs and control gates and does not identify which value binds each port.
- **Duplicate ordinary edges with inferred selection:** hides intent and changes previously invalid definitions into executable workflows.
- **Compiler-generated merge nodes:** can reuse some single-source structures but add artificial identities, lifecycle events, and provider coupling. Preserve groups directly in the input plan instead.
- **First-produced selection:** races candidates, misses later ambiguity, and changes the deterministic contract.

## Risks / Trade-offs

- **Single-source assumptions beyond validation** -> Audit inference maps, typed transfers, connectivity, descriptions, cache inputs, and compiled-plan generation; cover reordered candidates in parity tests.
- **Scope or frame leakage** -> Exercise changing winners across loop rounds, parallel iteration items, and stream frames, including all-skipped frames.
- **Incorrect skip observations** -> Use resolved logical-input outcomes for Task and operator observations; never infer consumer skipping from raw alternative edges.
- **Broader plan and description changes** -> Preserve default-empty serialization and old-definition fixtures, and update affected consumers in the same implementation group.
- **Reduced typed optimization near a group** -> Use an explicit conservative fallback; optimization is a separate concern from correctness.

## Migration and Validation

Existing workflows need no edits. Opt in by declaring one exclusive group at a common downstream input while keeping ordinary and control edges unchanged elsewhere. Add a self-contained `if_else` example using existing core nodes; the Git/CI tooling nodes are not present in this checkout and are not prerequisites.

Implement definition and plan support, shared resolution, generated and scoped execution, then observation and acceptance coverage. Verify unique, all-skipped, ambiguous, missing, false/null, dynamic type, secret, ordinary-gate, cancellation, and reordered-candidate cases. Include in-memory/compiled parity and collection-boundary regression cases. Use the repository's affected-test selector and pinned toolchain, and run repository hooks.

Rollback consists of removing opt-in groups and returning callers to supported graph structures, such as separate workflow entry points. Binaries using the new contract must be rebuilt with matching support packages; do not downgrade readers while retaining the new syntax. Integrating the earlier test-selection plan is a follow-up, not part of this implementation.
