## ADDED Requirements

### Requirement: Execute compiled graph layouts in generated runners

Generated runners SHALL bind initialized executors to immutable compiled dependency, node-index, output, execution-domain, and message-ownership data. Launch MUST NOT rebuild the graph, resolve graph node names into indices, partition execution domains, or derive message ownership. In-memory construction and generated execution SHALL share plan records and the same runtime scheduling implementation.

#### Scenario: Launch a generated fork and join

- **WHEN** a generated oneshot runner starts
- **THEN** its branch domains and join dependencies are loaded from constant plan data without graph construction

#### Scenario: Bind nested task bodies

- **WHEN** a generated Loop or Iteration is initialized
- **THEN** its body binds executors to the compiled body plan without repartitioning the graph

#### Scenario: Preserve dynamic execution state

- **WHEN** the same compiled graph is instantiated twice
- **THEN** immutable layout data is shared while contexts and mutable stream state remain independent
