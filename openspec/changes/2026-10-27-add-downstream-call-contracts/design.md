## Context and scope

`FlowConnection` describes concrete data-port transfers. `NodeMetadata` carries reflected port
contracts and internal planning information. The compiler resolves those contracts before execution,
while `ExecutionContext` already tracks the current node and invocation scope. These are the existing
boundaries to extend; node factories must continue to return complete prepared executors without
post-link initialization hooks.

In this design, Parent means the upstream caller, not necessarily a containing Flow. Dynamic dispatch
means choosing targets, arguments, and invocation counts from a statically prepared target set.
Discovery does not grant arbitrary executor access or mutate the prepared graph.

## Call and output model

The first implementation introduces standalone contracts in `mf-runtime`; it does not add a scheduler
entry point or change `TaskNode`, `NodeResult`, or existing workflow JSON.

`CallBinding` declares a public name, a scope-local target `NodeId`, explicit `CallPortMapping` entries,
and a `CallInputDefaults` policy. `ExplicitOnly` is the default; `RemainingInputs` explicitly requests
node-level fallback binding. Multiple caller outputs cannot claim the same destination within one
group. One output may map to multiple distinct inputs. Names are exact port identifiers, not field
paths. Construction validates local names and duplicate destinations; it does not claim to validate
graph-dependent types, aliases, external sources, or target eligibility.

`CallArguments` has two namespaces: `outputs` is keyed by explicitly mapped caller output names;
`defaults` is keyed by target input names supplied through the node-level default. This permits a
source output name and an unrelated target default input name to coincide. External producers never
contribute values to this caller-owned envelope. Shared `ValueRef` payloads and sensitivity survive
transport without conversion through JSON.

`CallRequest` explicitly selects a `CallBindingId` and atomically submits one argument group, even if
the group is empty. It is not an ordinary output map, an exclusive branch output, or a stream emission.
Ordinary result publication retains its existing behavior. A future dispatch interface must accept
requests before the Parent returns its final `NodeResult`, allowing calls and continuations within a
single Parent invocation. Requests must never be inferred merely from output presence.

`CallId` combines the caller-local binding index with an invocation number. The future dispatcher
must allocate invocation numbers uniquely throughout one workflow execution, including nested scopes
and stream frames. These numbers are not provider tool-call IDs or retry idempotency keys. Allocation,
pending-call tracking, duplicate completion detection, and cross-execution transport are not supplied
by the first contract-only implementation.

`CallRequest::respond` retains the request identity in `CallResponse`. `into_outcome(expected)` rejects
a different binding or invocation. `CallOutcome` distinguishes successful output values, a dependency
skip with structured causes, and failure with the original typed `WorkflowRunError`. An omitted optional
result remains absent. Loop scope effects are not part of the response envelope. A missing request for
another binding does not skip that binding or prevent a later request.

This borrows branch selection from IfElse but not `exclusive_outputs`: one call can carry several
parameters, and a Parent can select multiple targets or the same target repeatedly. The first graph
integration should reject a source port used for both ordinary publication and callable argument
publication until their combined lifecycle is explicitly defined.

## Port connections determine ownership

The second implementation adds programmatic resolution through `CompiledWorkflow::resolve_call_bindings`
and `FlowBuilder::resolve_call_bindings`. The former uses normal graph preparation and type inference,
deferring required-input checks only for declared targets in the root call scope. Nested scopes and
ordinary workflow preparation retain their existing validation. The latter accepts already prepared
metadata and retains the builder's ordinary scheduling behavior. Neither API creates an executable
call graph or changes existing JSON activation semantics.

Providers declare explicit caller slots in `NodeMetadata.call_outputs`, separately from ordinary and
stream outputs. Preparation rejects empty, duplicate, overlapping, or excessively nested declarations.
Call slots do not become ordinary output references or obligations of the final `NodeResult`.
Generated metadata checks include nonempty call slots without changing serialization for existing nodes.

Resolution produces runtime-owned `ResolvedCallBinding` records. Each `ResolvedCallInput` retains its
target contract and sensitivity, with an `Output`, `Default`, `External`, or `Omitted` source. Explicit
outputs retain their source contract as well. Runtime `FlowDependency` records preserve external data
and output-control dependencies; success dependencies and direct prerequisite node indexes are stored
separately. The compiler reuses ordinary type compatibility and exclusive-branch proofs, and rejects
waits on the caller or another target in the same callable set. These prerequisites are recorded, not
yet inserted into a running scheduler.

`bind_arguments` validates the two caller namespaces and maps them to target input names. External
inputs cannot be overridden, and failed validation returns no partial map. Optional omissions, nullable
values, lossless numeric checks, shared values, and sensitivity retain existing runtime semantics.
Source requiredness applies within the selected group; a value mapped to multiple inputs must satisfy
all destination contracts. The method returns only caller-owned inputs, leaving external resolution
and invocation activation to the subsequent runtime phase.

The initial resolver accepts ordinary terminal Task targets. It rejects external context references,
stdin, and startup environment/default bindings until invocation-scope support can preserve them.
TaskStream callers and terminal producer values are supported, while stream emissions are rejected
as external values. These restrictions fail during planning rather than silently dropping resources
or reading an unscoped latest value. Input sensitivity metadata is supported independently of startup
environment/default bindings.

The returned records are compiler/runtime planning data, not the future Parent-facing interface view.
Versioned JSON call syntax, executable plan integration, recursive call-graph validation across callers,
and generated call-binding tables remain deferred. Accepting runnable call syntax before on-demand
activation exists would incorrectly execute targets as ordinary roots.

Resolve explicit port bindings before considering node-level invocation connections:

| Connection | Input ownership |
| --- | --- |
| Parent port to target input port | Parent supplies the explicitly mapped input |
| Another producer port to target input port | That producer supplies the explicitly mapped input |
| Parent to target through a node-level invocation connection | Parent supplies remaining unbound inputs |
| Control or success dependency | No input ownership |

A port-level connection is authoritative only for its named destination input. Its existence does not
make the source responsible for every other target input. A node-level invocation connection supplies
defaults only; it never overrides an explicit port binding. Resolve this precedence at preparation
time, independently of runtime values and completion order.

For example, assume Search declares required `query`, `tenant_id`, and `token` inputs, and optional
`limit`:

```text
Parent.query   -> Search.query
Constant.value -> Search.tenant_id
Config.token   -> Search.token
```

In an explicitly declared callable binding containing the Parent port mapping, Parent sees only
`query`. Unbound `limit` remains omitted. If `limit` were required, preparation would reject the missing
binding. Adding a node-level Parent-to-Search invocation connection makes `limit` a Parent parameter;
its optionality is unchanged. With no explicit port bindings, that node-level connection assigns all
Search inputs to Parent.

Multiple explicit sources for one input retain the existing exclusive-binding validation and selection
rules. This precedence does not create last-writer-wins behavior. The initial callable contract rejects
an exclusive input group that mixes per-invocation caller arguments with external producer values;
such selection should be expressed inside a tool Flow instead. External-only exclusive groups retain
their existing semantics. Multiple node-level callers cannot compete to fill the same default binding
within one callable instance; separate instances or separately scoped callable bindings are required.

## Connection views and callable bindings

The third implementation provides caller-visible projections and explicit scope attachment.
`ResolvedCallBinding::interface()` retains the binding ID and public alias, then groups caller-owned
arguments into separate output and default namespaces. It never retains external or omitted inputs,
destination names for explicit mappings, node indexes, dependencies, resource configuration, or values.
The projected fields are immutable and serializable for inspection. Provider descriptions and complete
JSON Schema reflection remain later additions rather than inferred text or invented object fields.

A shared source output becomes one parameter whose requiredness and sensitivity are the OR of all
mapped obligations. Its deduplicated type constraints are conjunctive: both the source and every target
must accept the value. Keeping the conjunction avoids lossy widening and incorrect disjointness
assumptions, including the null overlap of different nullable types. Parameters sort by name and type
constraints by their descriptor display. Public validation and resolved input binding accept the same
caller values while reporting errors in their respective public and internal namespaces.

The compiler constructs `CallInterfaces` from a caller-indexed map of declarations. Normal scope
preparation runs once with the union of declared targets. The catalog uses definition IDs for caller
lookup, preserves local binding IDs, and rejects duplicate callers, IDs, and aliases. Distinct callers
may reuse IDs and aliases. Catalog debug output summarizes the caller count without dumping other
callers' interfaces. An empty catalog does not allocate storage.

Hosts explicitly attach catalogs to a `Flow` or `PreparedStream`, with known-task-owner validation.
`ExecutionContext::call_targets` exposes only the active node's slice. Graph-owned dispatch installs
its own catalog even when empty. Scoped guards restore the previous catalog and current-node identity
on normal return, errors, and panics. Domain/stream forks share immutable catalogs, while body forks
start empty. The same rules cover generated domain callbacks, snapshot fallbacks, combined producer
jobs, and isolated task execution. Same-named nodes in different prepared scopes remain independent.

This attachment is discovery-only: it does not attach executors, modify activation, or validate a live
dispatch capability. Existing ordinary nodes still follow their ordinary schedule. A prototype Flow
must not be executed with the expectation that catalog attachment makes tool targets lazy. Versioned
call definitions, generated catalog persistence, complete binding drift validation, and executable
invocation wiring remain deferred until the scheduling phase.

Keep three concepts distinct:

1. A node interface describes its full public input and output contract.
2. A resolved connection describes a concrete relationship and its destination port contract.
3. A callable binding groups one caller's argument mappings into an atomic invocation interface and
   identifies the target and external input bindings.

Ordinary port connections remain ordinary dataflow. They become per-invocation argument mappings only
inside an explicitly declared callable binding. A callable binding may contain only explicit port
mappings, or additionally enable node-level default binding. Declaring the group must not implicitly
enable the default. This distinction preserves existing fan-out behavior and supports partial callers
without requiring a node-level default connection.

Use a caller-local public alias for tool selection and an opaque prepared binding identity for runtime
dispatch. Parameter names default to target input names; explicit source-port mappings do not require
the source and destination names to match. Alias and parameter collisions are preparation errors.

Public views include node identity, execution kind, public descriptions, and the relevant resolved
port contracts. Store interfaces once and reference them from connection indexes. Do not clone full
`NodeMetadata` into each edge or serialize it into model requests: literal derivations, internal
references, defaults, and credentials are not public tool descriptions.

Conceptual context accessors are `downstream(output)` for direct data-port consumers and
`call_targets()` for explicitly callable bindings. Their exact Rust signatures are implementation
details. Both provide read-only, deterministically ordered views. Metadata inspection neither reads
target outputs nor establishes reverse execution dependencies.

## Preparation and scope binding

Preparation performs these steps:

1. Prepare complete nodes and reflect their contracts.
2. Resolve explicit port bindings, including existing exclusive groups.
3. Expand node-level invocation defaults into the remaining target inputs.
4. Validate input completeness, source conflicts, dependencies, and callable eligibility.
5. Complete type resolution and build immutable public interface projections.
6. Bind indexes to the prepared scope and expose the current caller's views during invocation.

Caller parameters without an independent producer type use the destination's accepted input type.
Do not invent output-value evidence for them. Metadata discovery cannot modify node ports or trigger
a reverse inference pass; this avoids a new fixed-point dependency between caller and target.

The runtime views must use final resolved contracts, rather than factory metadata captured before type
refinement. Generated plans and startup preparation must agree on the binding layout and public
contracts. Public contract changes must participate in the relevant generation and drift checks.

Views are local to a prepared scope. Nested Flow internals are visible only through their public
interface. Runtime identities include scope identity; a bare node index is not globally meaningful.
Forked domains share immutable scope indexes, while child invocations install their own indexes and
restore the caller's view on return. Dynamic tasks, generated bodies, and TaskStream callers must see
equivalent contracts.

## Composed invocation execution

Observation descriptions add version `2026-10-28` and an `Invocation` path variant carrying caller ID,
binding ID, selected target ID, and invocation count. It uses existing Call start/finish lifecycle
events while retaining the target's ordinary node events. The count is independent of Loop pass limits.
Receivers validate target identity, route identity, and nested caller continuity instead of treating
repeated tool calls as repeated root-node completions.

Virtual body templates use `LoopBodyDescription.call_targets`. Each template references its physical
graph's node and dependency contracts, plus caller-local binding/alias/target mappings. Lookup resets
to that physical scope after a dynamic invocation; ordinary Loop/Call entries still descend into child
graphs. This permits reusable nested call paths without exponential description expansion. Template
copies do not increase the static executor count. Built-in container nodes cannot own directed calls;
their body nodes own those declarations, avoiding ambiguity with the container's static body.

Finite and stream receivers retain invocation-specific outcomes, support reordered/duplicate evidence,
and reject a node event claiming a different selected target. TUI navigation reuses templates and
shows invocation, target, and binding identity. Runtime emission is connected in the subsequent dispatch
implementation; defining the observation protocol does not activate any tool.

The initial callable targets are terminal Tasks, including suitable terminal Flow wrappers. Tool
instances are activated on demand and excluded from ordinary root activation. Reject an instance that
mixes ordinary DAG activation and callable activation in the initial implementation.

One invocation submits the caller-owned parameter set atomically. The runtime assigns an invocation
identity, validates names and types, and resolves external input bindings before executing the target.
Unknown arguments and attempts to supply externally owned ports fail before target execution.
Required caller parameters must be present; optional omission and explicit null remain distinct.

External inputs use committed values visible in the caller invocation's execution scope. Preparation
must ensure their producers can complete before the call needs them, lowering necessary prerequisites
into the caller's execution plan and rejecting cycles. It must not block waiting for a producer that
can run only after the caller finishes. Exact skip and exclusive-source selection follow existing
input resolution semantics; a missing or failed source never changes parameter ownership or falls
back to caller arguments. A skipped invocation is reported to the caller as a structured outcome.

Externally bound values are captured consistently for the caller invocation. Calls never combine
arguments from different invocations or use an unscoped latest-value cache. For per-call configuration
refresh, put the configuration service inside the callable Flow so each call creates a complete child
scope. External producers remain ordinary scheduled graph work and may run even if no tool is selected;
putting them inside the callable Flow also provides lazy execution.

Execute targets through runtime validation, scope, budget, cancellation, and observation boundaries,
not direct calls to an exposed executor. Reuse the relevant mechanisms from isolated `NodeTarget`
execution and `ExecutionContext::run_call`, but do not reuse the assumption that the caller supplies
all inputs. Tool results belong to the invocation and return to its caller; they must not publish a
parent-graph completion for the reusable target instance or activate its ordinary successors.

Unselected targets produce no invocations and do not delay completion. Calls to the same prepared
instance are initially serialized; independent invocation contexts do not imply that provider-owned
state is fresh or that repeated calls are pure. Providers requiring fresh state need an explicit
construction policy before they are eligible. Reuse the shared worker mechanisms for nested work and
verify progress with a worker limit of one. Reject recursive callable bindings initially and enforce
runtime depth, step, deadline, and cancellation limits across child calls.

A thread-bound `WorkerResource` reservation starts before waiting for the executor lock, rejects
recursive entry, and outlives the lock guard. While any reservation is active, workers do not help
unrelated queued jobs and blocking child submissions execute inline on the submitting thread. This
also applies to an external caller holding an executor while using the pool. Queuing nested work
could otherwise place it behind callers waiting for that executor, including cross-thread wait cycles.
Completed child results are consumed before helping unrelated work.

This first implementation trades parallelism inside a tool invocation for predictable progress:
ordinary callers retain parallel scheduling, while a tool's nested work shares its current thread.
Dispatch must hold the reservation across lock acquisition and execution. Nonblocking `try_submit`
retains its queue-only contract and must not be used for children awaited while holding a reservation.
The worker prerequisite itself does not activate tools or serialize node instances.

## Schema and LLM boundary

The caller parameter schema is the full target input contract projected onto caller-owned inputs.
External inputs are absent from this schema. Node purpose and parameter descriptions remain public,
while values and internal binding details do not become model-visible metadata.

`ValueType::Object` does not describe nested record fields. Add richer schema reflection at the value
contract boundary where possible, retaining `ValueType` for port compatibility. Initially support only
accurately describable tool contracts; require explicit additional contracts for opaque objects.
Check structural agreement with the reflected ports, but do not claim arbitrary handwritten JSON
Schema can be proven equivalent to custom codecs. Runtime decoding and business validation remain
authoritative. Provider schema conversion must not silently change omission, null, or numeric semantics.

LLM integration consumes the generic callable view and maps public aliases back to prepared bindings.
It owns provider tool descriptions, structured call/result history, and bounded multi-round dialogue.
It dispatches only complete, validated tool calls, including when arguments arrive incrementally.
The runtime owns execution and returns typed outcomes; the LLM layer decides how supported failures
are represented in tool-result messages. Cancellation and exhausted budgets remain terminal control
outcomes, not opportunities to continue an unbounded model retry loop.

Preserve sensitivity metadata when composing values. Result export to the model needs an explicit
policy for sensitive values and size limits. Invocation identifiers support correlation but do not
guarantee idempotency; side-effecting tools must not be retried automatically after ambiguous failures.

## Alternatives and risks

- Exposing full target inputs ignores composition and lets callers overwrite external configuration.
  Projecting the resolved ownership set makes the advertised interface match executable inputs.
- Treating every explicit connection as a parameter binding confuses control edges with port edges.
  Only port bindings claim named inputs; node-level invocation links provide remaining defaults.
- Converting ordinary fan-out into selective invocation changes existing scheduling semantics.
  Explicit callable bindings keep ordinary dataflow stable.
- Link callbacks that mutate prepared node contracts violate complete preparation and risk inference
  cycles. Immutable views preserve the existing node construction boundary.
- Direct executor dispatch bypasses resource limits, observation, and input composition. Runtime
  invocation preserves those boundaries but needs careful worker reentrancy and result isolation.
- Duplicated schema declarations can drift. Shared reflection, startup checks, and runtime validation
  reduce this risk without pretending that all custom codecs are statically understood.
- New public metadata fields affect Rust struct literals and generated contracts. Use explicit schema
  versioning for authoring changes and rebuild providers against matching runtime contracts.

## Delivery and remaining implementation decisions

Version `2026-10-28` introduces a scope-local `calls` map, keyed by caller ID and alias. Each value has
`target`, optional explicit `ports`, and `defaults` (`explicit_only` by default or `remaining_inputs`).
Node-level fallback remains opt-in even for zero-port groups. Normal node input bindings provide
external values. Call declarations are preserved in Loop/Iteration bodies and resolved nested Flow
definitions; older scope versions reject the field. Canonical plans sort mappings by destination and
source, and bind aliases in sorted map order.

`CompiledCallLayout` freezes node identities, resolved private bindings, and the workflow identity.
The versioned JSON form uses duplicate-key rejection and typed port descriptors. Verification compares
against a freshly prepared layout, including definition identity, types, requiredness, sensitivity,
external sources, and local routes. This is planning evidence, not an executor capability. Public
interfaces are obtained through projection and omit external configuration values.

This intermediate PR deliberately rejects executable preparation and code generation for any directed
call definition. It also rejects preparing a nested callable body that would otherwise become an
ordinary executable subgraph. Thus accepting syntax cannot accidentally run tool nodes as roots.
The runtime PR must replace these guards with on-demand scheduling and global readiness validation.
Compiler declarations already reject recursion, unknown endpoints, duplicate caller mappings, and
targets used as ordinary producers or workflow outputs.

Deliver the call/output model first, then connection and ownership resolution. Derive caller-visible
interfaces from those resolved bindings and implement composed runtime invocation against the same
contracts. Interface exposure and runtime execution can proceed independently once resolution is
stable. Integrate Parent dispatch and LLM ToolUse afterward. No existing workflow becomes callable
merely by upgrading.

Before implementation, choose the exact versioned workflow encoding for callable groups and node-level
default links, the structural schema representation, and the observation encoding for repeated tool
invocations. These choices must preserve the ownership and activation semantics above; they are not
reasons to reinterpret existing data or control edges.
