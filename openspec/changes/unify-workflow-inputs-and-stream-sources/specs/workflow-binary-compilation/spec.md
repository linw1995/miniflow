# Spec Delta

## MODIFIED Requirements

### Requirement: Validate workflow structure before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs, existing edge endpoints, selected
output
node references and names, and acyclicity. Before installing the executable, its validation mode SHALL
validate
registered kinds, configuration, existing ports, required input connections or promoted initial-node
parameters, and at most one connection per input. Promotion SHALL apply only to top-level initial nodes in
schema `2026-10-03`; noninitial nodes, nested bodies, and older schemas SHALL retain their existing
required-edge rules.
Validation mode MUST NOT call node execution methods. The normal mode SHALL execute statically generated
orchestration.
Compiler validation SHALL accept statically safe assignments from refined types to the same type, compatible
refined
collection types, legacy broad supertypes, or `Any`. It SHALL also accept broad or `Any` outputs feeding a
refined input
when the source value is unknown and the shared runtime validates the actual value before invoking the target.
Known source values MUST be checked against target port types during validation; a known mismatch MUST fail
before
installation even when its inferred source descriptor is broad. Disjoint concrete types and incompatible
collection
shapes MUST fail validation; no implicit coercion SHALL occur.
Validation failures MUST include diagnostics that identify the relevant definition node, port, output, or
edge, and MUST NOT produce a successful binary.
The system MUST produce a deterministic topological execution order, using ascending definition ID to break
ties between ready nodes.
Port validation SHALL use a node instance's complete configuration-dependent port description when supplied,
and otherwise its static registration. Port names MUST be nonempty and unique within each direction. Base
descriptions and output derivations MUST depend only on configuration. Resolved output types MAY additionally
depend on upstream data bindings and MUST be consistent between validation and execution for the same
definition and linked plugins. Every node and branch MUST be validated even when it will be skipped during
execution.
Graph structure and topological order SHALL include both existing data edges and explicit control edges.
Control edges
MUST reference an existing source output and target node, MUST NOT create target input bindings, and MUST NOT
contain
duplicate identical entries. Required data-input and type compatibility rules SHALL continue to apply to data
edges.
Plugin validation SHALL build a unique index of `${node_id}.${output_name}` for all effective source outputs
and check
all declared context references by exact qualified-ID lookup. Qualified-ID collisions MUST fail with both
source pairs
before execution, regardless of whether the conflicting outputs could be skipped. Each referenced producer
MUST be a strict ancestor of the consumer through explicit data or control dependencies. Context references
MUST NOT
implicitly add dependencies. Unordered, self, and descendant references MUST fail even when a topological
tie-break
would place the referenced node first.

#### Scenario: Reject a cyclic workflow

- **WHEN** dependency edges in a definition form a cycle
- **THEN** compilation fails and reports a closed path containing only nodes in the cycle

#### Scenario: Stable order for independent nodes

- **WHEN** multiple nodes are ready to execute at the same point in a DAG
- **THEN** the compiler orders them by ascending definition ID regardless of node or edge declaration order

#### Scenario: Reject an invalid port connection

- **WHEN** an edge references a missing output or input port, or the connected port types are incompatible
- **THEN** compilation fails and reports both endpoints and the validation reason

#### Scenario: Validate a runtime-checked connection

- **WHEN** a source port of type `Any` feeds a target port of type `Int64`
- **THEN** compilation accepts the edge and the runner checks the actual JSON value before invoking the target

#### Scenario: Reject an incompatible refined connection

- **WHEN** a `List(String)` output feeds a `List(Int64)` input
- **THEN** compilation rejects the edge with both endpoints and their types

#### Scenario: Reject a known constant conflict

- **WHEN** a constant value `42` feeds a `String` input, directly or through an identity node
- **THEN** runner validation fails before installation and identifies both edge endpoints and their types

#### Scenario: Reject a nested known conflict

- **WHEN** a constant value `[1, "x"]` feeds a `List(Int64)` input on an inactive branch
- **THEN** runner validation fails without executing the branch and reports the failing path `/1`

#### Scenario: Keep an unknown broad source checked

- **WHEN** a plugin with no derivation exposes `Any` and feeds an `Int64` input
- **THEN** validation accepts the edge and execution checks the actual value at the target

#### Scenario: Reject an incomplete or ambiguous input

- **WHEN** a required input has neither a permitted startup parameter binding nor a data connection, or an
  input has more than one connection
- **THEN** compilation fails and identifies the node and input port

#### Scenario: Reject an invalid selected output

- **WHEN** a selected workflow output references an unknown node or output port, or repeats an output name
- **THEN** compilation fails and identifies the selected output

#### Scenario: Validate separate instances of one dynamic kind

- **WHEN** two instances of `builtin.if_else` declare different branch IDs and branch counts
- **THEN** each instance's edges and selected outputs are checked against its own derived ports

#### Scenario: Reject malformed instance port descriptions

- **WHEN** a plugin supplies an empty port name or duplicate input or output names
- **THEN** validation fails with the node and invalid descriptor context

#### Scenario: Activate a router without a data input

- **WHEN** a conditional node has valid incoming control dependencies and valid ancestor context references
- **THEN** validation succeeds without requiring a payload or condition input port

#### Scenario: Reject an unordered context reference

- **WHEN** a predicate references an existing output from a node outside the consumer's explicit ancestor
  chain
- **THEN** validation identifies the consumer, branch, and source and requires an explicit dependency instead
  of adding one

#### Scenario: Reject self or future references

- **WHEN** a predicate references its own node or a downstream node
- **THEN** validation fails before executable installation even if that predicate follows an always-matching
  branch

#### Scenario: Reject an unknown referenced output

- **WHEN** a predicate references a qualified output ID absent from the effective output index
- **THEN** validation reports the reference and its consumer branch before execution

#### Scenario: Reject ambiguous output identities

- **WHEN** effective output pairs `(a.b, c)` and `(a, b.c)` both form `a.b.c`
- **THEN** validation reports the collision before executable installation without splitting or resolving the
  key heuristically

#### Scenario: Include control dependencies in structural planning

- **WHEN** data and control edges together form a cycle, or a control edge names an unknown endpoint or
  duplicates an existing control edge
- **THEN** structural validation fails before runner generation

#### Scenario: Allow transitive source references

- **WHEN** `load_order` precedes `audit`, `audit` precedes `route`, and a route predicate reads
  `load_order.value`
- **THEN** validation accepts the reference without requiring another direct edge from `load_order` to `route`

#### Scenario: Validate every configured predicate

- **WHEN** a later branch has malformed predicate syntax, an invalid path escape, or an invalid literal for
  its operator
- **THEN** validation fails with node and branch context even if an earlier branch could match every input

#### Scenario: Validate an unselected branch

- **WHEN** a downstream node on an unselected branch has invalid configuration or a nonexistent port binding
- **THEN** build validation fails without invoking any node execution method

#### Scenario: Validate without startup values

- **WHEN** a new-schema initial task or stream producer declares required input ports
- **THEN** compilation validates and exposes those parameter requirements without executing the node or
  requiring invocation values during the build

### Requirement: Opt in to a typed streaming definition

Schema `2026-10-03` SHALL accept `execution` with `mode: "stream"` and optional resource limits, and MUST
reject `execution.input_type`. It SHALL derive startup parameters from initial nodes and obtain stream item
types from explicit source output ports without injecting `%input`. Without `execution`, the new schema SHALL
use single-run execution with the same startup parameter interface. Older accepted single-run schemas SHALL
retain their existing behavior; older streaming definitions MUST receive an explicit-source migration
diagnostic.

#### Scenario: Compile a typed stream

- **WHEN** a new-schema workflow connects an explicit integer source's `item` output to Batch
- **THEN** validation resolves integer items and list-of-integer batch output without an engine input node

#### Scenario: Preserve an older definition

- **WHEN** a definition uses an existing accepted single-run schema without streaming configuration
- **THEN** its execution, required connections, dependencies, and single-result output behavior are unchanged

#### Scenario: Require explicit streaming support

- **WHEN** a single-run definition contains an event or stream executor
- **THEN** validation fails with a mode diagnostic

#### Scenario: Migrate old streaming syntax

- **WHEN** a definition uses the old streaming schema, `execution.input_type`, or a reference to the removed
  synthetic `%input` source
- **THEN** compilation explains how to use schema `2026-10-03`, explicit sources, and startup parameters
  instead of silently changing activation behavior

#### Scenario: Reserve the input source

- **WHEN** a user node or plugin attempts to redefine the removed synthetic `%input` source
- **THEN** validation rejects the reserved legacy identity and directs the author to a named explicit source

### Requirement: Validate activation and message boundaries before installation

Streaming validation SHALL recognize initial tasks and stream producers without requiring a path from an
engine input node. It SHALL bind their declared inputs through the workflow interface and validate ordinary
startup dependencies. It MUST reject initial event executors, mixed-domain dependencies, cross-domain context
references, and mixed-domain selected outputs, including inactive branches. Graph, configuration, resource,
and type checks MUST run without executing nodes or reading input data.

#### Scenario: Activate a configured source per message

- **WHEN** a configured constant has a control edge from an explicit source's `item` output
- **THEN** it is activated within each emitted frame and its output stays in that message domain

#### Scenario: Start an unattached task once

- **WHEN** a streaming graph contains an initial task with valid startup bindings
- **THEN** it executes once in the startup frame without an external trigger

#### Scenario: Reject an unattached root

- **WHEN** an unattached root has an event executor whose contract defines no startup callback
- **THEN** validation requires an explicit activation dependency instead of inventing an input or timer event

#### Scenario: Reject a control edge across a batch boundary

- **WHEN** a batch-domain node is also gated by an upstream item-domain control output
- **THEN** validation reports the incompatible domains even if the value types otherwise fit

#### Scenario: Join initial task outputs

- **WHEN** two initial tasks feed an ordinary consumer in the startup frame
- **THEN** validation accepts the shared frame and preserves deterministic dependency order

### Requirement: Deliver streaming results without waiting for EOF

Each selected stream result SHALL be written and flushed as one JSON line without building a final aggregate.
Slow output MUST apply message-count backpressure, and output failure MUST fail execution. In stream mode,
runner transport MUST reserve result stdout and direct plugin stdout diagnostics to stderr, including
construction diagnostics.

#### Scenario: Deliver a complete batch early

- **WHEN** the first batch finishes while its explicit source remains active
- **THEN** its result line becomes available immediately to the output consumer

#### Scenario: Preserve result framing with a noisy plugin

- **WHEN** a plugin prints diagnostics during construction and execution
- **THEN** stdout still contains only workflow result JSON lines and diagnostics appear on stderr

#### Scenario: Fail a broken output pipe

- **WHEN** the result sink closes before the runner has delivered all selected outputs
- **THEN** the instance fails and the runner exits unsuccessfully

### Requirement: Describe streaming requirements without starting an instance

New streaming graph descriptions SHALL identify execution mode, required observation protocol, and support for
interface inspection without exposing business values or configuration. Their separate interface document
SHALL describe startup parameters and runtime input resources with matching workflow identity. `--describe`,
`--describe-interface`, and `--validate` MUST NOT consume source input, invoke executors, arm timers, or emit
workflow execution events.

#### Scenario: Validate with an idle open stdin

- **WHEN** a streaming runner is invoked with `--validate` while stdin remains open
- **THEN** validation completes without waiting for input or executing any source or Batch callback

#### Scenario: Inspect a streaming runner

- **WHEN** a standalone runner's graph and startup interface are inspected
- **THEN** the host can determine execution mode, required parameters, resource ownership, and observation
  compatibility before launching execution

## ADDED Requirements

### Requirement: Inspect configured startup interfaces in the installed runner

New runners SHALL support `--describe-interface`, returning one versioned JSON document with workflow
identity, startup input types/required flags, and resource requirements. It SHALL use linked provider
preparation with isolated stdout and no business execution. Existing `--describe` MUST remain factory-free.
Both operations SHALL work without build inputs or a second compilation.

#### Scenario: Inspect dynamic input metadata

- **WHEN** an external factory defines configuration-dependent root input ports
- **THEN** interface inspection reports the complete effective input contract without calling node execution

#### Scenario: Preserve clean output with noisy factories

- **WHEN** a factory writes diagnostics during interface inspection
- **THEN** stdout remains one complete interface document and construction diagnostics go to stderr

#### Scenario: Preserve one-build standalone inspection

- **WHEN** the validated executable is moved without source, plugin files, or a toolchain
- **THEN** both graph and interface inspection still work using that same executable

### Requirement: Accept startup parameters through shared execution arguments

Runners and `mf run` SHALL accept mutually exclusive `--inputs <JSON>` and `--inputs-file <PATH>`, with
omission meaning an empty object. Inspection/validation modes MUST reject execution arguments. CLI parameter
transport SHALL be bounded to 1 MiB, preserve exact parsed values, and avoid interpreting parameter JSON as
stream data. Relative files SHALL resolve from the invocation directory.

#### Scenario: Start a file producer with a path parameter

- **WHEN** standalone and TUI launches receive the same valid nested startup argument object
- **THEN** both invoke the same initial nodes with equivalent values and selected results

#### Scenario: Read a parameter file once

- **WHEN** the TUI reads an inputs file and that user file changes before child execution
- **THEN** the child receives the values already validated by the CLI rather than rereading the changed file

#### Scenario: Reject excessive or conflicting arguments

- **WHEN** parameter transport exceeds 1 MiB or both parameter flags are supplied
- **THEN** launch fails with an argument diagnostic before node execution

### Requirement: Route only declared input resources

Streaming runners SHALL obtain data through explicit source nodes and SHALL supply only their declared runtime
input resources. Runners MUST reserve result stdout before plugin construction and isolate diagnostics. A
workflow with no stdin source MUST start and complete independently of stdin. EOF SHALL close only its owning
source; result delivery remains incremental and backpressured.

#### Scenario: Execute with null stdin

- **WHEN** an autonomous file producer has valid startup parameters and no stdin resource requirement
- **THEN** the standalone runner executes with null stdin and emits its complete result sequence

#### Scenario: Keep one source active after stdin EOF

- **WHEN** a graph has an explicit stdin source and a separate active producer
- **THEN** stdin EOF drains only that source and does not declare the entire workflow complete

## REMOVED Requirements

### Requirement: Read JSON Lines input while timers progress

**Reason**: JSON Lines ingestion now belongs to an explicit stdin StreamNode rather than every streaming
runner. Its framing, type validation, timer independence, and failure behavior are preserved by the
`stream-sources` capability.

**Migration**: Add `builtin.stdin` with `config.item_type`, connect its `item` output in place of
`%input.item`, and use schema `2026-10-03`. Standalone callers can continue piping JSON Lines into that
runner. Autonomous sources use workflow startup arguments and require no stdin reader.
