## ADDED Requirements

### Requirement: Compile provider-dependent stream layout

The generated Cargo build SHALL resolve linked provider registrations before emitting stream layout constants. Execution kind, message boundaries, and domain partitioning MUST come from provider construction contracts rather than built-in kind-name assumptions. The build and executable SHALL use matching provider dependencies and features.

#### Scenario: Compile an external producer

- **WHEN** an external package returns a Stream executor
- **THEN** the generated build emits its message boundary and execution-domain membership before the executable starts

#### Scenario: Reject invalid static construction

- **WHEN** provider metadata or workflow graph validation fails during generated construction
- **THEN** Cargo build fails before installation and preserves prior executable and dependency-lock guarantees
