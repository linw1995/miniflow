## ADDED Requirements

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

## MODIFIED Requirements

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
