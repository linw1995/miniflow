# workflow-inputs Specification

## Purpose

Expose the input requirements of initial workflow nodes as a shared, typed startup interface for embedded
callers, standalone runners, and terminal launches.

## Requirements

### Requirement: Derive startup inputs from initial nodes

For schema `2026-10-03`, top-level nodes with no incoming data or control edges SHALL be initial nodes. Every
input port of an initial task or stream producer SHALL become a workflow startup parameter with its configured
type and required flag. Parameter bindings MUST NOT create graph dependencies or require a duplicate
user-authored input declaration.

#### Scenario: Start a parameterized producer

- **WHEN** an initial file producer declares a required string input named `path`
- **THEN** the workflow interface exposes that requirement and compilation succeeds without an upstream data
  edge or an actual path value

#### Scenario: Start an ordinary task with inputs

- **WHEN** an initial identity task declares a required input named `input`
- **THEN** the workflow exposes that input and can invoke the task once with a supplied startup value

#### Scenario: Derive configuration-dependent requirements

- **WHEN** two initial instances of one plugin kind expose different configured input ports
- **THEN** each instance contributes its own complete input contract

### Requirement: Preserve exact parameter identity

Startup values and interface declarations SHALL use a nested map keyed first by the exact node ID and then by
the exact port name. Same-named ports on different initial nodes MUST remain distinct. Nodes without
parameters SHALL expose an empty input declaration and SHALL accept omission of their value object.

#### Scenario: Keep two path arguments separate

- **WHEN** initial nodes `read` and `write` both declare `path`
- **THEN** the caller supplies independent values under `read.path` and `write.path` as nested object members

#### Scenario: Preserve punctuation in names

- **WHEN** a node ID or port name contains dots, slashes, or tildes
- **THEN** binding uses the complete JSON member name without splitting it and diagnostics escape the
  corresponding JSON Pointer path

### Requirement: Keep noninitial and nested inputs explicit

Required inputs of nodes with incoming data or control edges SHALL still require data bindings. Unbound
optional inputs SHALL remain absent. Inputs of nested body nodes MUST NOT be promoted to the outer workflow;
their enclosing scope retains ownership. Context references MUST continue to require explicit upstream
dependencies.

#### Scenario: Reject a missing input behind a control edge

- **WHEN** a task is activated by a control edge but its required data input is unbound
- **THEN** compilation rejects the missing binding rather than creating a workflow parameter

#### Scenario: Keep body requirements local

- **WHEN** an initial Loop has a required outer variable and its body contains a node with an unbound required
  input
- **THEN** the outer variable becomes a startup parameter while the missing body binding still fails
  compilation

### Requirement: Validate all startup values before execution

Every invocation SHALL validate its complete startup argument object before executing any node or consuming
any source data. It MUST reject duplicate keys, unknown node or port names, invalid object shapes, missing
required values, and type violations. Errors SHALL identify the affected node, port, and nested path without
coercion.

#### Scenario: Reject one invalid argument across multiple roots

- **WHEN** one initial node has valid arguments and another is missing a required argument
- **THEN** startup fails before either node executes or any stream producer reads data

#### Scenario: Reject ambiguous JSON arguments

- **WHEN** an argument document repeats a node or port key
- **THEN** validation rejects it rather than keeping either occurrence

#### Scenario: Reject a nested type mismatch

- **WHEN** a startup parameter declared as a list of integer maps contains a string at `/0/count`
- **THEN** startup fails with the node, port, expected type, and nested mismatch path

### Requirement: Preserve omission and null semantics

An omitted optional parameter SHALL remain absent from node inputs. Explicit null SHALL be a supplied value
subject to its port type. The startup interface MUST NOT infer defaults, coerce values, or derive later
invocation schemas from values supplied by earlier runs.

#### Scenario: Omit an optional port

- **WHEN** the caller omits an optional string parameter
- **THEN** the node receives no binding for that port

#### Scenario: Supply null to a string port

- **WHEN** the caller supplies null for a string parameter
- **THEN** startup rejects the value even if the port is optional

### Requirement: Bind parameters once per workflow invocation

Startup values SHALL be isolated by workflow invocation and bound once to their initial nodes. Emitted stream
messages MUST NOT rebind initial nodes or implicitly broadcast startup outputs into other message domains.
Single-run and stream entry points SHALL use the same parameter validation and binding rules.

#### Scenario: Produce many records from one path

- **WHEN** an initial producer receives a file path and emits many records
- **THEN** it receives the path once and each record independently activates its downstream message processing

#### Scenario: Invoke the same plan with different parameters

- **WHEN** two workflow instances supply different arguments to the same initial node
- **THEN** each instance observes only its own startup values and neither changes the other's input contract

### Requirement: Inspect an interface without running the workflow

The system SHALL expose a versioned startup interface containing workflow identity, complete parameter declarations,
and runtime resource requirements. For new compiled runners, direct and command-based inspection SHALL return the
embedded build-time interface without constructing providers. Inspection MUST exclude argument values and node
configuration and MUST NOT execute nodes, consume source input, start timers, or emit execution events.
Invalid or incomplete metadata SHALL prevent launch.

#### Scenario: Inspect an external producer's dynamic ports

- **WHEN** a compiled workflow is inspected without its build inputs or the referenced input file
- **THEN** its interface reports the producer's configured input requirements without constructing providers or opening that file

#### Scenario: Reject mismatched interface metadata

- **WHEN** a launcher receives graph and interface records identifying different workflows
- **THEN** it rejects preflight before workflow execution

#### Scenario: Keep inspection independent of factory initialization

- **WHEN** a new runner's factory would require an unavailable runtime resource to initialize an executor
- **THEN** inspection still returns the embedded interface without initializing that executor, while validation and execution retain their ordinary construction failure behavior

### Requirement: Reject generated startup interface drift

New generated runners SHALL compare their freshly prepared startup schema with the embedded interface during validation and every execution invocation. Initial-node and port identities, types, required flags, and resource conditions MUST agree. Disagreement SHALL fail before executor invocation, source consumption, timers, or worker scheduling, with affected node/port/resource context. Dynamic in-memory workflows SHALL retain their existing preparation rules.

#### Scenario: Reject a changed input type

- **WHEN** a generated runner prepares an initial input with a type different from its embedded declaration
- **THEN** validation and execution fail with the affected node and port before any node executes

#### Scenario: Reject a changed required flag or port set

- **WHEN** runtime preparation changes an input's required flag, adds or removes a port, or changes the initial-node input declarations
- **THEN** the runner rejects the mismatch rather than executing with a different interface

#### Scenario: Reject changed stdin ownership

- **WHEN** runtime preparation changes unconditional or conditional stdin ownership from the embedded declaration
- **THEN** the runner rejects the mismatch before reading source data or starting workers

#### Scenario: Preserve an installed runner after validation mismatch

- **WHEN** a newly built runner's validation detects an interface mismatch before installation
- **THEN** compilation fails and preserves the previously installed executable

#### Scenario: Accept matching task and stream interfaces

- **WHEN** a generated task or stream runner prepares the same schema as its embedded interface
- **THEN** it continues with ordinary authoritative argument/resource validation and unchanged execution semantics

#### Scenario: Keep dynamic embedded callers independent

- **WHEN** an in-memory caller prepares a workflow without a generated executable
- **THEN** ordinary dynamic interface derivation and argument validation work without requiring an embedded manifest
