# Design

## Context

See [proposal.md](proposal.md) for the motivation and delivery scope.

The generated runner in `mf-compiler/src/dependency_project.rs` currently parses every invocation against
the initial-node schema in its embedded manifest, then selects finite or streaming execution at compile
time. `plan.rs` emits provider preparation, compiled layouts, and typed domain executors.

Preparation resolves node metadata through the original graph. Removing incoming edges before preparation
would change inferred contracts, particularly forwarding and collection output types. Task executors retain
a dynamic adapter even when ordinary graph execution uses a generated typed segment. Tasks inside a
prepared stream are accessible separately from the instance-owned Event and Stream states.

`ExecutionContext` binds startup values only to declared initial nodes, resolves data/control dependencies,
validates outputs, and owns observations and snapshots. Its context outputs distinguish a supplied value,
an explicit skip, and an absent key. Some tasks, including `builtin.if_else`, read declared context
references instead of named input ports. Existing observation counters use positions in an execution
prefix; invoking one middle node with the full description would misreport the visited prefix.

## Goals / Non-Goals

**Goals:**

- Reuse validated preparation and task execution boundaries across finite and streaming binaries.
- Preserve the target's original resolved port contract and initialize fresh invocation state.
- Make supplied inputs, external context, output presence, resources, and observation identity explicit.
- Keep normal generated execution and its public APIs compatible.

**Non-Goals:**

- Rebuild a smaller graph at launch or specialize types from supplied values.
- Avoid construction of unrelated providers in this first implementation.
- Add a general context mutation API, replay upstream executions, or reuse a prior run's state.
- Interpret Task selection as source-driven stream execution or start a stream instance to invoke a Task.
- Expand the manifest startup interface, restore runner inspection commands, or add TUI target selection.

## Decisions

### 1. Emit target records during the existing validated build preparation

Add deterministic top-level target records to the generated plan artifacts, serialized in build-generated
`node-targets.json` and embedded into the runner. Each record contains the
exact definition ID, compiled node index, execution category, resolved input/output contracts, declared
external context requirements, stdin condition, and the finite selected-operation description. Reference
types are resolved using the original graph's complete output identities, never by splitting dotted keys.
Records are derived from the same provider preparation that already emits layouts and the manifest.

At launch, reuse generated whole-plan preparation, then bind the selected record to its prepared task.
Compare its input/output identities, types, required flags, context references, and stdin condition with
the compiled record before invocation. Keep the existing whole-workflow startup drift check as well.
Unknown targets and unsupported execution categories can be rejected from the compiled records without
constructing providers. A selected prepared task's capability must still agree with its record.

This preserves inference and avoids another provider API or runtime graph planner. Generating a separate
singleton workflow was rejected because it loses original inference and context dependency validation.
Preparing only the target was deferred because it requires a separate reconstruction contract. Full-plan
preparation may fail on an unrelated factory; that remains an explicit limitation.

### 2. Parse runner controls separately from node values

Introduce a shared runner invocation representation containing an optional exact target, port arguments,
and optional external context. Support both `--node ID` and `--node=ID`, at most once. Token parsing must
respect argument/value boundaries; it must not remove every token matching a control flag. For example,
`--transform.amount=--node` is a string port value rather than a selector.

Without a selector, dispatch through the current startup schema and behavior. With a selector, parse
qualified port arguments against its compiled input schema. Keep existing string, nullable-string, JSON,
duplicate-key, transport-exclusion, and escaped-path rules. The target does not need unrelated root inputs,
including when compiled from a definition predating startup-input promotion.

Add mutually exclusive `--context JSON` and `--context-file PATH`; these may accompany either port-input
transport and require `--node`. Context is a flat object keyed by exact referenced output IDs. Treat null
as a value checked against the referenced type. Do not add skipped-value or previous-run replay syntax.
Require every declared external reference and reject undeclared context keys before invoking the target.

Bound port and context transports individually, and their combined canonical payload, to 1 MiB. Read files
once with a bounded reader; new context-file transport accepts regular files and rejects nonregular input
without waiting for FIFO writers. Validate optional omission, required inputs, and the target's stdin
condition before business execution. Diagnostics and conversions retain typed Snafu sources.

### 3. Invoke an isolated task through the shared lifecycle

Add a prepared-target execution entry point that accepts a prepared task and resolved target contract, with
validated port bindings, context seeds, and runtime options. It must work with tasks borrowed from either
a finite Flow or a prepared stream. Keep provider lookup and contract construction in the compiler's
preparation boundary; runtime execution does not need a registry.

Use a fresh execution context. Bind only the selected task's inputs and seed only validated external
references through a restricted initialization path. Do not fabricate producer executions or globally
relax context visibility. Reuse the ordinary per-node invocation, publication, error, step-budget, snapshot,
and observation machinery with no external execution dependencies.

Use the task's dynamic adapter, bypassing any enclosing typed segment and whole-graph domain scheduler.
Calling `execute_position` on the original Flow was rejected because it still resolves incoming
dependencies. Calling `TaskNode::execute` directly was rejected because it bypasses publication validation
and runtime lifecycle behavior.

A Loop or Iteration remains one selected outer operation; its already compiled body executes normally,
including limits, worker ownership, ordered writes, exits, and error policy. External context is not
promoted into bodies that ordinarily cannot read it. Nested-body IDs and synthetic scope controls cannot
be selected independently.

### 4. Return port results with finite execution transport

Collect all produced target outputs by their exact port names after normal publication validation. Omit
explicitly skipped ports, retain explicit null, and return `{}` for successful zero-output execution.
The workflow's declared output aliases and optional selections do not restrict target results.

Isolated Task invocation returns one JSON object and a success/failure exit status even when the containing
binary was compiled for streams. Reserve result stdout before provider construction and route plugin
diagnostics to stderr using the existing descriptor-ownership machinery adapted for a finite result.
Supply stdin only when the selected task requires it; do not activate unrelated sources. Do not claim
stream stdio by starting an otherwise unnecessary stream instance.

### 5. Describe the selected operation independently

Emit a finite observation description with the selected root at position zero, no external data/control
edges, and only the nested Loop structure required by that selected operation. Preserve local node IDs and
existing Iteration detail semantics. The descriptor exposes no configuration or business values.

Derive its workflow identity through the existing canonical identity encoder over an envelope containing
an isolation-format discriminator, the original workflow identity, and the exact target ID, with a
one-element top-level order. This distinguishes the selected operation from both whole-flow runs and other
targets, while supplied values do not change its identity. Emit original-workflow and target correlation
as span attributes; the ordinary embedded manifest continues to describe whole-flow execution.

Create observation before provider preparation and use the selected descriptor consistently for finite
lifecycle events and snapshot export. Preparation failures in unrelated providers are unattributed to an
executed node while retaining the actual provider ID in diagnostics. A successful Task has one visited
top-level node; nested Loop steps retain their normal counts. Snapshot eligibility follows the selected
finite operation, including selection from a streaming binary, rather than its containing binary feature.

Reusing the whole-flow observation was rejected because its visited-prefix semantics are incorrect for
a middle-node selection. Telemetry initialization, unavailable-export handling, and bounded shutdown retain
ordinary result and exit-status semantics.

### 6. Keep the first delivery boundary explicit

The change implements top-level Task executors and whole Loop/Iteration operations in both binary modes.
Selecting a producer, Event node, nested body node, or synthetic control reports an actionable unsupported
target diagnostic without executing business logic. No `mf run --tui --node` behavior is introduced.

The next producer phase can adapt singleton producer execution to the existing emitter, backpressure,
acknowledgment, cancellation, and NDJSON delivery contract. The Event phase must separately specify message
transport, input admission, serial callback order, timer progression, close delivery, drain, and outputs
before implementation. TUI selection additionally needs a versioned target-interface catalog and launch
preflight. These follow-ups must be recorded before declaring #125 complete for all node categories.

## Risks / Trade-offs

- Whole-plan preparation can initialize or fail unrelated factories -> document the limitation and test
  that unrelated business methods and source reads are never invoked.
- Original graph metadata and target contracts can drift -> emit both from one validated build and compare
  the selected freshly prepared contract before dispatch.
- Context injection could become an arbitrary output mutation surface -> validate exact declared keys and
  types at preparation/invocation boundaries and expose only restricted invocation initialization.
- A selected task could accidentally start producers or its typed neighbors -> use an independent finite
  dispatch path and counters in integration fixtures to prove isolation.
- Observation identity changes relative to whole-flow execution -> publish deterministic target identities
  and original-flow correlation while preserving the whole-flow manifest and protocol meanings.
- An all-skipped result is visually indistinguishable from no outputs -> document omission and preserve
  explicit port outcomes in opt-in lifecycle metadata and snapshots.

## Migration Plan

1. Add prepared-target records and runtime support with existing whole-flow APIs unchanged.
2. Wire new runner controls and finite target dispatch for both compiled binary modes.
3. Add acceptance coverage and documentation, then run focused and repository validation.
4. Recompile workflows to obtain the new mode. Existing executables and manifest readers keep their current
   behavior; no definition or lockfile migration is required beyond ordinary dependency resolution.
5. Reverting the feature and recompiling restores the previous runner interface. Do not archive this change
   until its implementation is verified, and do not close #125 as universally supported at this phase.
