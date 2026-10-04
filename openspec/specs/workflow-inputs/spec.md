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

The system SHALL expose a versioned startup interface containing workflow identity, complete parameter
declarations, and runtime resource requirements. Inspection MUST exclude argument values and node
configuration and MUST NOT execute nodes, consume source input, start timers, or emit execution events.
Invalid or incomplete metadata SHALL prevent launch.

#### Scenario: Inspect an external producer's dynamic ports

- **WHEN** a compiled workflow is inspected without its build inputs or the referenced input file
- **THEN** its interface reports the producer's configured input requirements without opening that file

#### Scenario: Reject mismatched interface metadata

- **WHEN** a launcher receives graph and interface documents identifying different workflows
- **THEN** it rejects preflight before workflow execution
