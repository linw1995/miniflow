## ADDED Requirements

### Requirement: Own workflow construction in the compiler

The compiler SHALL own workflow graph validation, domain partitioning, and workflow construction errors. The runtime SHALL consume validated executable plans and SHALL NOT depend on the compiler. Runtime execution APIs SHALL NOT reconstruct graphs or return workflow construction errors. Node factory contracts MAY remain runtime contracts without transferring workflow compilation responsibilities to the runtime.

#### Scenario: Build an executable workflow

- **WHEN** a caller constructs a workflow from an untrusted graph
- **THEN** compiler construction APIs validate the graph and return a runtime executable plan or a typed compiler construction error

#### Scenario: Bind a generated plan

- **WHEN** a generated runner initializes executors for its compiled layout
- **THEN** runtime binding consumes the validated layout without partitioning or validating a graph and without a compiler dependency
