# Spec Delta

## Purpose

This capability converts declarative DAG workflow definitions into deployable native executables that include the required node plugins and a fixed workflow execution plan.

## ADDED Requirements

### Requirement: Compile workflow definitions into executable binaries

The system SHALL provide a compilation entry point that reads workflow definitions with a `YYYY-MM-DD` date-formatted version and produces an executable workflow binary. The system MUST reject malformed or unsupported definition versions. The binary MUST encode the validated node order and port bindings as generated executable code, run that workflow when started, and require no source definition file at runtime.

#### Scenario: Compile a valid workflow

- **WHEN** a user compiles a structurally valid DAG definition that references registered node kinds
- **THEN** the system generates and compiles a runner and outputs an executable workflow binary

#### Scenario: Run a compiled workflow

- **WHEN** a user runs the generated binary
- **THEN** the system executes nodes according to the defined dependencies and returns the selected workflow outputs

### Requirement: Resolve plugins from the compile-time registry

The system SHALL resolve node kinds in a definition from the plugin registry visible to the compiler. A generated workflow binary MUST link the required plugin implementations. The iteration order of the plugin registry MUST NOT affect node resolution or workflow execution order.

#### Scenario: Compile with a linked plugin

- **WHEN** a definition references a node kind registered by a plugin linked into the current compilation target
- **THEN** the system creates a runtime instance of that node and includes its implementation in the generated binary

#### Scenario: Reject an unavailable plugin

- **WHEN** a definition references an unregistered node kind
- **THEN** compilation fails and identifies the node ID and unknown kind

### Requirement: Validate workflow structure before code generation

Before generating a runner, the system SHALL validate nonblank unique node IDs, existing edge endpoints and ports, required input connections, at most one connection per input, valid selected outputs, and acyclicity.
A concrete output type SHALL connect only to an input of the same type or type `Any`; an output of type `Any` SHALL connect only to an `Any` input.
Validation failures MUST include diagnostics that identify the relevant definition node, port, output, or edge, and MUST NOT produce a successful binary.
The system MUST produce a deterministic topological execution order, using ascending definition ID to break ties between ready nodes.

#### Scenario: Reject a cyclic workflow

- **WHEN** dependency edges in a definition form a cycle
- **THEN** compilation fails and reports a closed path containing only nodes in the cycle

#### Scenario: Stable order for independent nodes

- **WHEN** multiple nodes are ready to execute at the same point in a DAG
- **THEN** the compiler orders them by ascending definition ID regardless of node or edge declaration order

#### Scenario: Reject an invalid port connection

- **WHEN** an edge references a missing output or input port, or the connected port types are incompatible
- **THEN** compilation fails and reports both endpoints and the validation reason

#### Scenario: Reject an incomplete or ambiguous input

- **WHEN** a required input has no connection or an input has more than one connection
- **THEN** compilation fails and identifies the node and input port

#### Scenario: Reject an invalid selected output

- **WHEN** a selected workflow output references an unknown node or output port, or repeats an output name
- **THEN** compilation fails and identifies the selected output

### Requirement: Report build failures

The system SHALL report generated-code or Cargo compilation failures as workflow compilation failures and preserve sufficient diagnostics to locate the error. The system MUST NOT report partially generated files as a successful final binary.

#### Scenario: Cargo compilation fails

- **WHEN** the generated runner or a plugin dependency fails to compile
- **THEN** the compile command returns a failure status and displays the relevant Cargo diagnostics
