# Workflow Runtime Execution Delta

## MODIFIED Requirements

### Requirement: Execute only complete validated plans

FlowRuntime SHALL only accept prepared executable Flows, prepared stream plans, or validated prepared Task
targets selected from those plans. Construction SHALL validate executor kinds, dependencies, graph ordering,
workflow output selections, execution-domain partitioning, and message ownership before returning a plan.
Target preparation SHALL retain the original resolved contracts and validate isolated bindings without
weakening the source graph's validation. Execution entry points MUST NOT instantiate nodes, validate graph
structure, or construct domain plans.

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

#### Scenario: Execute a prepared isolated target

- **WHEN** a caller supplies a prepared Task target with validated isolated input and context bindings
- **THEN** execution uses its original resolved contracts without requiring a registry or running external dependencies

### Requirement: Execute compiled graph layouts in generated runners

Generated runners SHALL bind initialized executors to immutable compiled dependency, node-index, output,
execution-domain, and message-ownership data. Launch MUST NOT rebuild the graph, resolve graph edge names
into indices, partition execution domains, or derive message ownership. Isolated selection SHALL resolve
an exact target ID through a build-emitted target record containing its compiled index, without deriving
a new graph. In-memory construction and generated execution SHALL share plan records and the same runtime
scheduling implementation; isolated Task invocation SHALL share the per-node lifecycle.

#### Scenario: Launch a generated fork and join

- **WHEN** a generated oneshot runner starts
- **THEN** its branch domains and join dependencies are loaded from constant plan data without graph construction

#### Scenario: Bind nested task bodies

- **WHEN** a generated Loop or Iteration is initialized
- **THEN** its body binds executors to the compiled body plan without repartitioning the graph

#### Scenario: Preserve dynamic execution state

- **WHEN** the same compiled graph is instantiated twice
- **THEN** immutable layout data is shared while contexts and mutable stream state remain independent

#### Scenario: Select a compiled middle task

- **WHEN** a runner selects a noninitial Task by its exact definition ID
- **THEN** dispatch uses its existing compiled index and resolved contract without removing edges or repartitioning domains

## ADDED Requirements

### Requirement: Reject isolated target contract drift

Generated isolated invocation SHALL compare its prepared target with the compiled target contract before
business execution. Executor category, port identities, resolved types, required flags, declared external
references, and stdin conditions MUST agree. Drift MUST fail with affected node, port, or resource context
and preserve the underlying typed error causes where present.

#### Scenario: Reject changed output metadata

- **WHEN** a prepared target exposes a different output type from its build-emitted target contract
- **THEN** selection fails before invoking the target with node and output context

#### Scenario: Reject changed context or resource requirements

- **WHEN** the prepared target changes a declared external reference or stdin condition
- **THEN** selection fails before reading input resources or invoking business logic
