# node-preparation Specification

## MODIFIED Requirements

### Requirement: Use one task execution entry point

Ordinary tasks SHALL implement one execution method receiving resolved inputs and a mutable `ExecutionContext`
and returning `NodeResult`. Tasks within one execution domain SHALL share that context serially, including
previously committed outputs in the domain. Concurrent domains SHALL receive isolated contexts seeded from
committed predecessor results and visible scope state. The runtime SHALL validate and publish each task result
before dependent work proceeds, and commit domain effects once before scheduling dependent domains.

#### Scenario: Execute a context-aware task

- **WHEN** a conditional task reads an explicit predecessor output
- **THEN** the runtime provides that committed value through its domain context and publishes the validated result

#### Scenario: Keep concurrent domain contexts isolated

- **WHEN** independent domains execute at the same time
- **THEN** neither domain can observe or mutate the other's uncommitted outputs or context state

#### Scenario: Commit a staged scope mutation

- **WHEN** a task stages a Loop scope mutation and its domain completes successfully
- **THEN** the runtime commits the mutation once before scheduling dependent domains
