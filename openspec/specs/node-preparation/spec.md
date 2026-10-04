# node-preparation Specification

## Purpose

Separate configured node metadata and construction from runtime execution, so executors are complete before use.

## Requirements

### Requirement: Return complete prepared nodes

Node factories SHALL return a prepared node containing configuration-derived metadata and its execution
implementation. Metadata SHALL include ports, output derivations, and declared context references.
The compiler SHALL validate and resolve this metadata before execution. Execution traits MUST NOT
require metadata hooks or methods that complete a partially constructed node.

#### Scenario: Prepare dynamic ports

- **WHEN** a provider derives ports from its configuration
- **THEN** its factory returns those ports as metadata and compilation validates them without executing the task

### Requirement: Use one task execution entry point

Ordinary tasks SHALL implement one execution method receiving inputs and the current mutable execution
context and returning `NodeResult`. Context access, explicit skips, scoped control, and ordinary outputs
SHALL use that method. The runtime SHALL retain dependency checks and validated publication around it.

#### Scenario: Execute a context-aware task

- **WHEN** a conditional task reads an explicit predecessor output
- **THEN** the runtime invokes its task method with the current context and publishes the validated result

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
