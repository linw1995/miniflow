# node-preparation Specification

## Purpose

Separate configured node metadata and construction from runtime execution, so executors are complete before use.

## Requirements

### Requirement: Return complete prepared nodes

Node factories SHALL return a prepared node containing metadata reflected from the executor's input and output
contracts and its complete execution implementation. Metadata SHALL include ports, output derivations, and declared
context references. The compiler SHALL validate and resolve this metadata before execution. Execution traits MUST NOT
require hooks that complete a partially constructed node. Reflected input contracts and resource conditions MUST remain
stable between generated-build and runtime preparation for the same configuration and selected provider, including
across host and target implementations. Runtime resource initialization MAY fail without changing these contracts.

#### Scenario: Prepare dynamic ports

- **WHEN** a provider prepares a checked program or validated dynamic schema
- **THEN** its factory reflects ports from that execution contract and compilation validates them without executing the node

#### Scenario: Preserve configured declarations across environments

- **WHEN** the same configured provider is prepared during generated compilation and runner startup in different process environments
- **THEN** it reports the same input contracts and resource conditions while retaining independent executor state

#### Scenario: Preserve declarations across host and target builds

- **WHEN** host and target builds use platform-specific executor initialization
- **THEN** their reflected input contracts and resource conditions agree

#### Scenario: Keep initialization failures separate

- **WHEN** a provider cannot initialize an executor because a required runtime resource is unavailable
- **THEN** preparation reports the construction failure without substituting a different input contract

### Requirement: Use one task execution entry point

Ordinary tasks SHALL implement one execution method receiving resolved inputs and a mutable `ExecutionContext`
and returning `NodeResult`. Tasks within one execution domain SHALL share that context serially, including
previously committed outputs in the domain. Concurrent domains SHALL receive isolated contexts seeded from
committed predecessor results and visible scope state. The runtime SHALL validate and publish each task result
before dependent work proceeds, and commit domain effects once before scheduling dependent domains.

#### Scenario: Execute a context-aware task

- **WHEN** a conditional task reads an explicit predecessor output
- **THEN** the runtime provides that committed value through its domain context and publishes the validated result

#### Scenario: Keep concurrent domain contexts isolated

- **WHEN** independent domains execute at the same time
- **THEN** neither domain can observe or mutate the other's uncommitted outputs or context state

#### Scenario: Commit a staged scope mutation

- **WHEN** a task stages a Loop scope mutation and its domain completes successfully
- **THEN** the runtime commits the mutation once before scheduling dependent domains

### Requirement: Declare factory construction requirements

Registrations SHALL distinguish plain factories from factories requiring prepared subgraphs. A subgraph
factory SHALL receive its body and execution options during construction. A request missing a required
body or supplying a body to a plain factory MUST fail during preparation without returning an executor.

#### Scenario: Require a prepared body

- **WHEN** a caller constructs a Loop or Iteration without its required prepared body
- **THEN** preparation fails before any runnable node is returned

#### Scenario: Preserve third-party registration

- **WHEN** a selected external package registers multiple kinds using the new preparation contract
- **THEN** inventory discovery and generated execution resolve those kinds through the same runtime identity

### Requirement: Select execution kind during preparation

A prepared node SHALL contain a task, event, or stream executor alongside its metadata. Event and stream
providers SHALL construct their state directly and MUST NOT require a task execution method. Tasks SHALL
retain `Send + Sync`; event and stream state MAY be `Send` without `Sync` and SHALL be invoked through
exclusive mutable access. The event contract SHALL accept input, timer, and upstream-close events and
return complete emissions and deadline updates. Stream executors SHALL emit results incrementally
during each startup or input invocation. An initial EventNode without upstream dependencies MUST be
rejected because the event contract does not define autonomous startup.

#### Scenario: Construct event state directly

- **WHEN** a factory returns an event implementation containing Send-only mutable state
- **THEN** preparation succeeds without a task adapter or an additional state-construction hook

#### Scenario: Construct producer state directly

- **WHEN** a factory returns a stream implementation containing Send-only mutable state
- **THEN** preparation exposes its metadata without invoking the producer

#### Scenario: Reject an autonomous event node

- **WHEN** an event executor is placed at the workflow root without any incoming dependency
- **THEN** preparation requires an explicit activation source and does not invent an initial input or timer
  event

### Requirement: Restrict synchronous execution to tasks

Synchronous flows and generated task bodies SHALL contain only task executors. Event and stream nodes MUST be rejected during preparation of a synchronous flow, before any executor is invoked. Metadata inference SHALL remain available independently of the execution kind.

#### Scenario: Reject an event in a synchronous flow

- **WHEN** a caller supplies a prepared event node to synchronous flow construction
- **THEN** construction fails with the definition identity and does not invoke the event

#### Scenario: Reject a producer in a synchronous scope

- **WHEN** a stream node appears in a synchronous flow, Loop body, or Iteration body
- **THEN** preparation fails with its definition identity before executing the producer

### Requirement: Declare execution resources during preparation

Prepared metadata SHALL declare at most one stdin requirement per node: unconditional ownership or ownership
unless a named input is supplied. Data bindings and validated startup arguments SHALL resolve conditional
ownership before execution. Validation SHALL reject competing active owners without acquiring input.
Discovery MUST use provider metadata without built-in kind-name inference or a separate factory contract.

#### Scenario: Prepare an external stdin provider

- **WHEN** a third-party stream provider declares exclusive runtime stdin
- **THEN** its resource requirements are available for validation and launch without reading stdin

#### Scenario: Reject conflicting ownership

- **WHEN** two prepared nodes actively require exclusive use of the same input resource
- **THEN** validation reports both consumers and the resource before execution

### Requirement: Separate construction and execution failures

Construction APIs SHALL return construction error types retaining typed sources. Runtime execution error types MUST NOT contain node construction, graph construction, or compiler failures. Compiler orchestration APIs MAY expose separate preparation and execution variants. Generated preparation SHALL report construction failures with node or scope attribution without converting them into execution errors.

#### Scenario: Preserve an embedded configuration error

- **WHEN** embedded node configuration cannot be decoded
- **THEN** preparation returns a construction error retaining the decode error source

#### Scenario: Launch a prepared stream

- **WHEN** the runtime starts a successfully prepared stream
- **THEN** launch can report input, runtime configuration, or resource failures but cannot report a compiler failure

### Requirement: Own workflow construction in the compiler

The compiler SHALL own workflow graph validation, domain partitioning, and workflow construction errors. The runtime SHALL consume validated executable plans and SHALL NOT depend on the compiler. Runtime execution APIs SHALL NOT reconstruct graphs or return workflow construction errors. Node factory contracts MAY remain runtime contracts without transferring workflow compilation responsibilities to the runtime.

#### Scenario: Build an executable workflow

- **WHEN** a caller constructs a workflow from an untrusted graph
- **THEN** compiler construction APIs validate the graph and return a runtime executable plan or a typed compiler construction error

#### Scenario: Bind a generated plan

- **WHEN** a generated runner initializes executors for its compiled layout
- **THEN** runtime binding consumes the validated layout without partitioning or validating a graph and without a compiler dependency

### Requirement: Prepare typed tasks from input and output definitions

Typed task preparation SHALL derive both port directions from owned structs and preserve derivations, context references, and resource requirements. Separate input or output declarations MUST fail with a typed construction error, including identical declarations. Preparation MUST NOT execute business logic, encode values, or require invocation arguments.

#### Scenario: Prepare a typed task without arguments

- **WHEN** a factory prepares a typed task with a required string and an optional shared JSON field before invocation values exist
- **THEN** its metadata contains the derived input and output ports without invoking the task or converting invocation values

#### Scenario: Reject competing declarations

- **WHEN** a caller supplies input or output port metadata to typed preparation, even if it matches the corresponding struct
- **THEN** preparation returns a typed construction error instead of overriding either declaration

#### Scenario: Preserve provider metadata

- **WHEN** a typed identity provider declares output forwarding and a typed provider declares context references or conditional stdin ownership
- **THEN** preparation retains those declarations for ordinary compiler and runtime validation

### Requirement: Adapt typed task execution within the runtime

The runtime SHALL decode dynamic resolved inputs, invoke typed business logic, and encode outputs required at dynamic
boundaries as ordinary task results. Eligible generated typed segments SHALL be able to transfer validated fields
directly between invocations. Explicit skipped names and loop summaries SHALL be preserved. Encoding required at a
boundary MUST complete before publication. Existing threading, dependency resolution, skip precedence, input checks, and
output checks SHALL retain their execution boundaries.

#### Scenario: Invoke typed business logic

- **WHEN** an active typed task receives inputs satisfying its declared struct
- **THEN** runtime conversion supplies the input struct, business logic executes once with the existing mutable context,
  and outputs required at dynamic boundaries are encoded

#### Scenario: Skip without decoding

- **WHEN** existing dependency rules skip a typed task
- **THEN** neither struct conversion nor typed business logic executes

#### Scenario: Preserve missing dependency precedence

- **WHEN** a typed task has one skipped dependency and another unexpectedly missing output
- **THEN** the missing dependency error is reported before struct decoding or business execution

#### Scenario: Validate typed task outputs

- **WHEN** typed business logic returns an output that contradicts its prepared declaration
- **THEN** execution fails before publishing that task's result through equivalent output checks on both dynamic and
  generated typed paths

#### Scenario: Preserve typed execution control metadata

- **WHEN** a typed result emits explicit skips, present null data, or a loop summary
- **THEN** adaptation preserves all three independently of output value encoding

#### Scenario: Reject encoding before publication

- **WHEN** one typed output encodes successfully but a later output cannot be encoded
- **THEN** workflow execution fails with the producer and typed cause and publishes none of that result

#### Scenario: Invoke an eligible typed successor directly

- **WHEN** an eligible generated task receives validated required fields from its typed predecessor
- **THEN** it executes once with a directly constructed input struct and the same node lifecycle and context boundaries

### Requirement: Integrate typed inputs with existing preparation consumers

Typed task metadata SHALL participate in compiler validation, startup interfaces, manifest comparison, and generated execution through the existing prepared-node contract. Derived declarations MUST remain independent of invocation values and process state. Dynamic task, event, and stream providers SHALL remain supported through reflected contracts without adopting typed execution.

#### Scenario: Reject an incompatible typed edge

- **WHEN** a concrete string output is connected to a derived integer input
- **THEN** ordinary compiler validation rejects the connection before execution

#### Scenario: Expose typed startup parameters

- **WHEN** an initial typed task has required, optional, or renamed struct fields
- **THEN** its workflow interface exposes the derived port names, descriptors, and required flags using existing argument validation rules

#### Scenario: Freeze generated typed declarations

- **WHEN** a generated runner prepares a typed task whose startup declarations differ from its embedded interface
- **THEN** existing manifest comparison rejects launch before business execution or source consumption

#### Scenario: Use typed tasks in both execution modes

- **WHEN** a typed task executes in a synchronous flow or a stream message domain
- **THEN** the same prepared adapter supplies typed inputs and preserves the corresponding execution context

#### Scenario: Retain configuration-dependent inputs

- **WHEN** a dynamic provider validates configuration into its execution input contract
- **THEN** it reflects that contract through `NodePortContract` and retains map execution without an input struct

### Requirement: Refine typed output descriptors without competing declarations

Typed tasks MAY narrow reflected outputs using separate output evidence. Evidence MUST reference existing ports,
preserve declared names and required flags, and admit only values allowed by the reflected descriptor. Invalid or
duplicate evidence MUST fail compiler preparation before business execution. Proven types MUST respect the shared
descriptor-depth limit. Actual produced values SHALL retain ordinary runtime publication checks.

#### Scenario: Refine a shared list from a prepared body

- **WHEN** a typed task collects shared values from a body declaring signed integer results
- **THEN** reflection declares the shared-list field and separate evidence resolves it to `List(Int64)`

#### Scenario: Reject competing output declarations

- **WHEN** a factory supplies an output port list alongside its reflected value contract
- **THEN** construction fails even when the supplied names, requiredness, and descriptors are identical

#### Scenario: Reject an invalid output refinement

- **WHEN** evidence references an unknown output, duplicates an output derivation, widens its descriptor, or exceeds the depth limit
- **THEN** compiler preparation fails without invoking business logic

### Requirement: Integrate typed outputs through existing execution consumers

Struct-defined outputs SHALL participate in ordinary compiler connection validation and in-memory and generated task execution, including stream task domains. Dynamic task providers SHALL retain their map result interface. Typed providers SHALL adopt owned result declarations without requiring workflow definition changes or node-specific compiler guards.

#### Scenario: Reject a disjoint derived output connection

- **WHEN** a derived integer output connects to a concrete string input
- **THEN** compilation rejects the connection before execution

#### Scenario: Match generated and in-memory typed outputs

- **WHEN** a typed task executes in-memory or in a generated synchronous or streaming runner
- **THEN** both paths encode equivalent JSON and reject invalid floating outputs before publication

### Requirement: Prepare tasks from unified value contracts

Typed preparation SHALL accept unified named-port values in either task role and derive their ports from the canonical
declaration. It SHALL preserve other metadata, reject competing declarations, and retain typed construction errors.
Output type evidence SHALL be separate from reflection and validated by ordinary compiler inference. Preparation MUST
NOT invoke business logic or convert invocation values.

#### Scenario: Prepare the same value contract in both roles

- **WHEN** a task uses the same unified struct for input and output and provides forwarding metadata
- **THEN** preparation derives both port directions and retains forwarding without decoding, encoding, or executing the task

#### Scenario: Preserve output refinement validation

- **WHEN** a unified output contract has a type proven by configuration or a prepared body
- **THEN** reflection retains the canonical declaration and compiler inference validates and resolves the separate evidence

### Requirement: Advertise optional typed generation contracts

Providers SHALL be able to advertise a typed generation contract alongside ordinary preparation. The generated typed
constructor SHALL agree with the ordinary factory's configured ports, derivations, context references, and resources.
Absent generation information SHALL retain dynamic execution; malformed supplied information SHALL fail build validation
with provider and node context.

#### Scenario: Retain a dynamic provider

- **WHEN** a provider has an ordinary factory and no typed generation contract
- **THEN** generated runners prepare and execute it through the existing dynamic path

#### Scenario: Expose a provider-owned typed shim

- **WHEN** an external provider keeps its executor private and exports a typed construction and invocation shim
- **THEN** generated execution can use the shim without exposing or naming the private executor

#### Scenario: Reject contradictory generated metadata

- **WHEN** the advertised typed constructor disagrees with the ordinary factory's configured interface
- **THEN** build validation fails before installation without invoking business logic

#### Scenario: Preserve initialization failures

- **WHEN** the generated typed constructor cannot initialize a runtime resource
- **THEN** preparation returns a source-bearing construction failure with node attribution before workflow execution

### Requirement: Share one provider instance across generated invocation strategies

Generated typed execution and its dynamic fallback SHALL use the same prepared task instance and resource ownership.
Preparation MUST NOT construct a second executor solely for fallback, generation inspection, or manifest production.
Selecting a runtime observation strategy MUST NOT reinitialize provider state.

#### Scenario: Switch to snapshot fallback

- **WHEN** the same prepared workflow executes first through its typed path and later with payload snapshots
- **THEN** both invocations use the same initialized task instance and preparation does not acquire resources again

### Requirement: Reflect contracts across executor kinds

Typed task, event, and stream constructors SHALL reflect both port directions from execution-associated value types.
Dynamic constructors SHALL reflect validated execution contracts through `NodePortContract`. Constructors MUST reject
factory port declarations, preserve other metadata, and avoid invoking execution methods. Separate low-level assembly
of an existing `NodeExecution` and metadata MAY remain available.

#### Scenario: Reflect fixed event and stream bags

- **WHEN** an event or stream provider associates fixed value types with its typed execution methods
- **THEN** construction reflects both directions automatically without a `NodePortContract` implementation

#### Scenario: Reflect a checked dynamic program

- **WHEN** a CEL provider has validated input conversion types and checked output programs
- **THEN** reflected ports match those same contracts used to bind inputs and encode results

#### Scenario: Reject duplicate sources of truth

- **WHEN** a task, event, or stream factory supplies input or output port metadata beside its executor's contract
- **THEN** construction fails before execution even if the metadata matches the reflected contract

#### Scenario: Preserve preparation-only metadata

- **WHEN** a reflected provider declares conditional stdin ownership, context references, or output evidence
- **THEN** construction preserves those fields without reading inputs, dispatching events, or running business logic

### Requirement: Prepare typed event and stream execution from associated value contracts

Typed event and stream preparation SHALL derive both port directions from the types used by business execution.
It SHALL preserve other metadata and reject competing declarations without requiring `NodePortContract`.
Preparation MUST NOT decode invocation values, dispatch events, acquire text inputs, or emit outputs.

#### Scenario: Prepare a fixed producer without reading input

- **WHEN** a Readline provider declares typed optional path input and typed line output
- **THEN** preparation exposes those ports and conditional stdin ownership without opening a file or reading stdin

#### Scenario: Preserve event collection evidence

- **WHEN** a typed Batch provider declares collection evidence
- **THEN** preparation reflects its input/output types and preserves the independent evidence for ordinary inference

### Requirement: Adapt typed event callbacks at the runtime boundary

Runtime adaptation SHALL decode input events before business dispatch and encode all returned emissions before
publishing any effect. Timer and upstream-close callbacks MUST NOT decode input fields. Batch metadata, explicit skips,
loop summaries, timer updates, and buffer observations SHALL be preserved through the same initialized state.
Empty typed effects MUST NOT require a default output value.

#### Scenario: Reject input before mutating event state

- **WHEN** a required typed input is absent or invalid
- **THEN** the adapter returns a typed decode error without invoking the event method

#### Scenario: Dispatch controls without invocation values

- **WHEN** a timer or upstream-close callback reaches a typed event requiring input fields
- **THEN** the control event executes without inventing or decoding input arguments

#### Scenario: Reject a later invalid emission

- **WHEN** an event returns one encodable result followed by an invalid floating output
- **THEN** the adapter returns the typed encoding cause and exposes none of the returned effects

#### Scenario: Preserve event observations and state

- **WHEN** typed event execution retains shared data and reports buffer, batch, or timer information
- **THEN** dynamic adaptation preserves payload identity and observations without constructing another state object
