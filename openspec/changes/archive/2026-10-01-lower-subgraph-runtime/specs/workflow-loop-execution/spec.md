## MODIFIED Requirements

### Requirement: Declare bounded Loop containers

`mfn-core` SHALL register `workflow.loop`. The system SHALL accept Loop containers only in the `2026-09-29` workflow definition version. A Loop
SHALL declare a nonempty typed variable set, a body DAG, and a maximum pass count from 1 through
1000. Each variable SHALL be a required Loop input and required Loop output of the declared type.
The body SHALL have a synthetic `$loop` source exposing current variable values and a zero-based
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
- **THEN** runner validation fails before installation and identifies the missing kind
