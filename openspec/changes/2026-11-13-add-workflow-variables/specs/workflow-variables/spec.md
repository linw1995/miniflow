## ADDED Requirements

### Requirement: Declare typed invocation-local state

Definition version `2026-11-13` SHALL support one engine-owned `workflow.variables` declaration per
finite callable Flow. Each variable SHALL have a unique nonblank name, a port type descriptor, and a
required compatible initial literal. The declaration SHALL expose typed outputs without startup
inputs or an intrinsic index, and MUST NOT have incoming dependencies or executable children.

#### Scenario: Declare a nullable value

- **WHEN** a variable declares a nullable string type and an explicit null initial value
- **THEN** the declaration is valid and exposes a null value initially
- **AND** omitting the initial field instead fails preparation

#### Scenario: Reject invalid declarations

- **WHEN** declarations are duplicated, variables are empty or duplicated, or an initial value conflicts with its type
- **THEN** preparation fails before any node executes
- **AND** descriptor and type failures retain their typed sources

### Requirement: Assign through explicit execution dependencies

`workflow.assign` SHALL take a required typed `value` input and publish `done` only after validation
and successful write publication. A reached assignment SHALL overwrite its current graph's variable
and refresh the declaration output. A skipped or failed assignment MUST NOT change it. Consumers
requiring a write SHALL explicitly depend on the assignment; ordinary cached outputs SHALL not change.

#### Scenario: Read a committed update

- **WHEN** a reader consumes the declaration output and depends on `assign.done`
- **THEN** it sees the assigned value in both interpreted and generated execution
- **AND** selecting the declaration output at Flow completion yields the final value

#### Scenario: Preserve a skipped assignment

- **WHEN** an inactive branch skips an assignment
- **THEN** the variable retains its previous value and `done` is skipped

#### Scenario: Reject a broad source with an incompatible runtime value

- **WHEN** an any-typed source supplies an invalid nested value to an assignment
- **THEN** execution fails without committing a write or completion output
- **AND** the original typed mismatch remains inspectable

### Requirement: Isolate state lifetime and ownership

Every invocation SHALL initialize fresh root state and release it on success, failure, or unwinding.
Root state MUST NOT add Loop frames, conceal startup inputs, or grant cross-scope access. Callable
child Flows SHALL own independent state. Loop assignments SHALL target only current Loop variables;
Iteration inputs SHALL remain immutable. Root declarations MUST NOT authorize Loop exits.

#### Scenario: Reuse a prepared Flow

- **WHEN** the same prepared Flow is executed twice
- **THEN** both executions begin with initial values rather than the previous final state

#### Scenario: Call a stateful child from a stateful parent

- **WHEN** parent and child both declare a variable with the same name
- **THEN** child writes cannot change the parent's value
- **AND** the child's observations remain in the existing Call scope

### Requirement: Preserve deterministic finite execution

Root-state Flow domains SHALL execute serially, preserving committed native values and sensitivity.
Explicit or factory-selected top-level streaming with root variables SHALL be rejected. Root state
instructions MUST NOT be offered as isolated node targets. Existing ordinary Flows SHALL retain
their parallel scheduling and typed execution behavior.

#### Scenario: Reject streaming state

- **WHEN** a root-state Flow selects an incremental output or contains an autonomous streaming node
- **THEN** preparation rejects the unsupported execution mode before executing nodes

### Requirement: Freeze authoritative state relationships

Compiled graphs and generated runners SHALL retain and validate root source, types, initial values,
and write targets without reconstructing them from provider labels. New state kinds SHALL be
reserved against plugin registration and rejected on earlier definition versions.

#### Scenario: Execute without authoring sources

- **WHEN** a generated root-state runner executes after its source file has been removed
- **THEN** it returns the same final state as in-memory execution

#### Scenario: Reject a corrupt compiled root relation

- **WHEN** stored relationships contain an unknown source, colliding instruction roles, undeclared target, or invalid initial value
- **THEN** preparation and description reject the artifact before provider execution
