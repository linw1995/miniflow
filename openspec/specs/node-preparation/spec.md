# node-preparation Specification

## Purpose

Separate configured node metadata and construction from runtime execution, so executors are complete before use.

## Requirements

### Requirement: Return complete prepared nodes

Node factories SHALL return a prepared node containing configuration-derived metadata and its execution
implementation. Metadata SHALL include ports, output derivations, and declared context references.
The compiler SHALL validate and resolve this metadata before execution. Execution traits MUST NOT
require metadata hooks or methods that complete a partially constructed node. Configured input declarations and
resource conditions MUST remain stable between generated-build and runtime preparation for the same configuration
and selected provider, including across host and target implementations. Runtime resource initialization MAY fail
without changing these declarations.

#### Scenario: Prepare dynamic ports

- **WHEN** a provider derives ports from its configuration
- **THEN** its factory returns those ports as metadata and compilation validates them without executing the task

#### Scenario: Preserve configured declarations across environments

- **WHEN** the same configured provider is prepared during generated compilation and runner startup in different process environments
- **THEN** it reports the same input declarations and resource conditions while retaining independent executor state

#### Scenario: Preserve declarations across host and target builds

- **WHEN** host and target builds of a selected provider use platform-specific executor initialization
- **THEN** their configured input declarations and resource conditions agree

#### Scenario: Keep initialization failures separate

- **WHEN** a provider cannot initialize its executor because a required runtime resource is unavailable
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

### Requirement: Prepare typed tasks from one input definition

Typed task preparation SHALL derive all input port names, types, and required flags from the task's input struct. It SHALL preserve caller-supplied output declarations, derivations, context references, and resource requirements. Supplying separate input declarations to typed preparation MUST fail with a construction error. Preparation MUST NOT execute business logic or require invocation values.

#### Scenario: Prepare a typed task without arguments

- **WHEN** a factory prepares a typed task with a required string and an optional shared JSON field before invocation values exist
- **THEN** its metadata contains a required string port and a non-required Any port without invoking the task

#### Scenario: Reject competing declarations

- **WHEN** a caller supplies input port metadata to typed preparation, even if it matches the input struct
- **THEN** preparation returns a typed construction error instead of overriding either declaration

#### Scenario: Preserve provider metadata

- **WHEN** a typed identity provider declares output forwarding and a typed provider declares context references or conditional stdin ownership
- **THEN** preparation retains those declarations for ordinary compiler and runtime validation

### Requirement: Adapt typed task execution within the runtime

The runtime SHALL decode resolved inputs into the declared struct and invoke typed business logic without provider-written map conversion. Typed tasks SHALL retain the existing task threading and execution contract. Existing dependency resolution, skip precedence, bound-input checks, and output publication checks SHALL apply before or after typed execution at their current boundaries.

#### Scenario: Invoke typed business logic

- **WHEN** an active typed task receives inputs satisfying its declared struct
- **THEN** runtime conversion supplies the struct and business logic executes once with the existing mutable execution context

#### Scenario: Skip without decoding

- **WHEN** existing dependency rules skip a typed task
- **THEN** neither struct conversion nor typed business logic executes

#### Scenario: Preserve missing dependency precedence

- **WHEN** a typed task has one skipped dependency and another unexpectedly missing output
- **THEN** the missing dependency error is reported before struct decoding or business execution

#### Scenario: Validate typed task outputs

- **WHEN** typed business logic returns an output that contradicts its prepared declaration
- **THEN** execution fails before publishing that task's result through the same output checks used by dynamic tasks

### Requirement: Integrate typed inputs with existing preparation consumers

Typed task metadata SHALL participate in compiler validation, startup interfaces, manifest comparison, and generated execution through the existing prepared-node contract. Derived declarations MUST remain independent of invocation values and process state. Legacy dynamic task, event, and stream providers SHALL remain supported without adopting typed inputs.

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

- **WHEN** a dynamic provider derives its input ports from configuration
- **THEN** its existing preparation and execution APIs continue to operate without an input struct
