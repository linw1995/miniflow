## ADDED Requirements

### Requirement: Execute only complete validated plans

FlowRuntime SHALL only accept prepared executable Flows or prepared stream plans. Construction SHALL validate executor kinds, dependencies, graph ordering, workflow output selections, execution-domain partitioning, and message ownership before returning a plan. Execution entry points MUST NOT instantiate nodes, validate graph structure, or construct domain plans.

#### Scenario: Build a generated oneshot workflow

- **WHEN** a generated workflow is prepared successfully
- **THEN** its execution APIs accept the prepared Flow and do not require a node registry

#### Scenario: Reject invalid construction before execution

- **WHEN** node construction or graph validation fails
- **THEN** preparation returns a construction error and no executor is invoked

#### Scenario: Prepare nested bodies

- **WHEN** a workflow contains Loop or Iteration bodies
- **THEN** their executable plans are constructed before the outer preparation succeeds

#### Scenario: Reject unresolved port references

- **WHEN** a constructor receives a data connection, control dependency, or output selection referencing an undeclared port
- **THEN** construction fails before execution, including direct Flow and prepared-stream construction APIs
