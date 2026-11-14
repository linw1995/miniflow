## ADDED Requirements

### Requirement: Declare typed invocation-local state

Definition version `2026-11-13` SHALL accept a definition-level `variables` list in each finite
callable Flow and MUST NOT require a declaration node. Compilation SHALL inject one canonical
synthetic `%root` source. Each variable SHALL have a unique nonblank name, a port type descriptor, and
a required compatible initial literal. The source SHALL expose typed outputs without startup inputs
or an intrinsic index, and MUST NOT be authored, receive dependencies, or own executable children.

#### Scenario: Declare a nullable value

- **WHEN** a variable declares a nullable string type and an explicit null initial value
- **THEN** the definition is valid and `%root` exposes a null value initially
- **AND** omitting the initial field instead fails preparation

#### Scenario: Reject invalid declarations

- **WHEN** a node uses the reserved source ID or kind, variables are empty or duplicated, or an initial value conflicts with its type
- **THEN** preparation fails before any node executes
- **AND** descriptor and type failures retain their typed sources

### Requirement: Reference scope values without declaring source nodes

`%root`, `%loop`, and `%iteration` SHALL be available in input bindings and output selections without
source node declarations. Authoring preparation MUST reject nodes using reserved source IDs or
internal source kinds. The compiler SHALL supply internal source instructions; compiled-artifact
replay SHALL use stored relationships without authoring lowering.

#### Scenario: Read and assign without a source declaration

- **WHEN** a Flow reads root variables, a Loop body assigns a current-frame variable, or an Iteration selects the current item without defining a source node
- **THEN** the appropriate scope values are available and execution succeeds
- **AND** the compiled plan can be saved and replayed with its internal source instructions intact

#### Scenario: Reject an explicit source declaration

- **WHEN** an authored node claims `%root`, `%loop`, `%iteration`, or an internal source kind, even with a canonical ID, kind, and empty config
- **THEN** preparation rejects the declaration with a diagnostic explaining that scope sources are implicit
- **AND** source-like JSON values in an ordinary node's opaque configuration are not interpreted as declarations

### Requirement: Assign through explicit execution dependencies

`workflow.assign` SHALL take a required typed `value` input and publish `done` only after validation
and successful write publication. A reached assignment SHALL overwrite its current graph's variable
and refresh the `%root` output. A skipped or failed assignment MUST NOT change it. Consumers
requiring a write SHALL explicitly depend on the assignment; ordinary cached outputs SHALL not change.

#### Scenario: Read a committed update

- **WHEN** a reader consumes the `%root` output and depends on `assign.done`
- **THEN** it sees the assigned value in both interpreted and generated execution
- **AND** selecting the `%root` output at Flow completion yields the final value

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
Iteration inputs SHALL remain immutable. Definition-level variables MUST NOT authorize Loop exits.

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
and write targets without reconstructing them from provider labels. `workflow.assign` and the
`%root` source name SHALL be reserved against plugin registration, and earlier definition versions
SHALL reject the field.

#### Scenario: Execute without authoring sources

- **WHEN** a generated root-state runner executes after its source file has been removed
- **THEN** it returns the same final state as in-memory execution

#### Scenario: Reject a corrupt compiled root relation

- **WHEN** stored relationships contain an unknown source, colliding instruction roles, undeclared target, or invalid initial value
- **THEN** preparation and description reject the artifact before provider execution
