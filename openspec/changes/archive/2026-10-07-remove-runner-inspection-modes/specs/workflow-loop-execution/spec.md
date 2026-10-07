## MODIFIED Requirements

### Requirement: Declare bounded Loop containers

`mfn-core` SHALL register `workflow.loop`. The system SHALL accept Loop containers only in the `2026-09-29` workflow definition version. A Loop
SHALL declare a nonempty typed variable set, a body DAG, and a maximum pass count from 1 through
1000. Each variable SHALL be a required Loop input and required Loop output of the declared type.
The body SHALL have a synthetic `%loop` source exposing current variable values and a zero-based
`index`. A Loop container SHALL resolve a `workflow.loop` declaration from the selected node
packages; the compiler SHALL bind the engine-prepared body to that registered implementation.
The node package SHALL own the sequential driver, state progression, termination conditions, and
completion summary. Assignment
and exit kinds and the synthetic source SHALL remain engine-owned and MUST NOT resolve through
plugin registrations. Earlier definition versions SHALL retain their existing behavior.

#### Scenario: Initialize and expose loop variables

- **WHEN** an upstream output is connected to a Loop variable input and a downstream node reads the Loop variable output
- **THEN** the body reads that initial value during its first pass and the downstream node receives the final value after Loop completion

#### Scenario: Reject an invalid definition version

- **WHEN** a `2026-09-26` definition contains a Loop construct or a `2026-09-29` Loop has an invalid maximum count or duplicate variable name
- **THEN** validation fails before runner installation and identifies the invalid field

#### Scenario: Reject a missing Loop declaration

- **WHEN** the selected node bundle does not register `workflow.loop`
- **THEN** build validation fails before installation and identifies the missing kind

### Requirement: Validate every loop body as a local DAG

The planner SHALL validate outer and body graphs independently. Body data and control edges SHALL
stay in their scope, except for reads from the synthetic `%loop` source. Ordinary body nodes SHALL
resolve from declared dependencies and undergo the same configuration, port, type,
context-reference, and required-input validation as top-level nodes. Context references SHALL
resolve only to same-scope explicit ancestors or `%loop`. Nested Loops SHALL be supported to depth
four; assignment and exit steps SHALL apply to the nearest enclosing Loop and SHALL be invalid
outside a Loop.

#### Scenario: Reject a body cycle or cross-scope reference

- **WHEN** a body contains a cycle, references a parent node without a Loop input, or connects an edge to a node in another scope
- **THEN** compilation fails with the Loop path and offending edge or reference

#### Scenario: Validate an inactive body branch

- **WHEN** a body branch would be skipped at runtime but contains an unknown plugin kind, invalid configuration, or incompatible port
- **THEN** build validation fails before executing any node

#### Scenario: Reject assignment outside a Loop

- **WHEN** an assignment or exit step appears at the top level or targets a variable absent from the nearest Loop
- **THEN** validation fails with that step and target context
