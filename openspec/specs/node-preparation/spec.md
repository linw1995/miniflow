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

A prepared node SHALL contain a task or event executor alongside its metadata. Event providers SHALL
construct event state directly and MUST NOT require a task execution method. Tasks SHALL retain
`Send + Sync`; event state MAY be `Send` without `Sync` and SHALL be invoked through exclusive mutable
access. The event contract SHALL accept input, timer, and upstream-close events and return complete
emissions and deadline updates.

#### Scenario: Construct event state directly

- **WHEN** a factory returns an event implementation containing Send-only mutable state
- **THEN** preparation succeeds without a task adapter or an additional state-construction hook

### Requirement: Restrict synchronous execution to tasks

Synchronous flows and generated task bodies SHALL contain only task executors. An event node MUST be
rejected during preparation of a synchronous flow, before any executor is invoked. Metadata inference
SHALL remain available independently of the execution kind.

#### Scenario: Reject an event in a synchronous flow

- **WHEN** a caller supplies a prepared event node to synchronous flow construction
- **THEN** construction fails with the definition identity and does not invoke the event
