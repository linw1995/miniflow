# Workflow Observability Delta

## ADDED Requirements

### Requirement: Identify isolated operations separately from whole-flow runs

Selected Task execution SHALL use a deterministic identity derived from the original workflow identity,
exact target ID, and isolated-operation format. Different targets and whole-flow execution MUST have
distinct identities; supplied values MUST NOT affect identity. Events, finite descriptions, and snapshots
SHALL agree on that identity. Observation SHALL retain original workflow and target correlation without
exposing configuration or business values automatically.

#### Scenario: Invoke one target with different values

- **WHEN** the same selected task runs twice with different inputs or context values
- **THEN** observations share its selected-operation identity and have distinct run identities

#### Scenario: Distinguish targets and execution modes

- **WHEN** callers execute the whole flow and select two different tasks from the same binary
- **THEN** the three operation descriptions and their events identify three distinct operations

#### Scenario: Correlate with the containing workflow

- **WHEN** a selected-operation span is exported
- **THEN** metadata identifies its original workflow and exact target without automatically exporting supplied values

### Requirement: Count only selected finite execution

Isolated Task observation SHALL describe the selected task at top-level position zero with no external
execution edges. Lifecycle counts MUST include only that operation and its actual nested work, without
claiming visits or skips for surrounding nodes. Existing Loop pass and Iteration detail meanings SHALL
remain unchanged. A Task selected from a stream binary SHALL use finite observation semantics.

#### Scenario: Observe a middle task

- **WHEN** a selected task originally appears after several predecessors
- **THEN** successful completion reports one visited top-level task and no lifecycle events for its neighbors

#### Scenario: Observe a selected Loop

- **WHEN** a selected Loop executes several passes
- **THEN** it has one top-level lifecycle and its real body invocations retain normal pass paths and step counts

#### Scenario: Select a finite task from a streaming binary

- **WHEN** an ordinary Task is selected from a stream flow
- **THEN** observation reports one finite operation without startup frames, stream message lifecycles, or producer events

### Requirement: Preserve isolated failure and snapshot boundaries

Selected execution SHALL preserve preparation, input, invocation, publication, and result-selection
failure phases. It MUST NOT emit a successful target start for rejected bindings or failed preparation.
Unrelated provider preparation failures MUST NOT count as target execution. Opt-in snapshots SHALL follow
the selected finite operation, including Task selection from stream binaries. Export or snapshot failures
MUST retain ordinary bounded cleanup and business-result semantics.

#### Scenario: Reject invalid isolated context

- **WHEN** selected context validation fails before invocation
- **THEN** any emitted failure identifies input or preparation context and no target-start event is emitted

#### Scenario: Fail unrelated factory preparation

- **WHEN** whole-plan preparation fails in a provider outside the selected operation
- **THEN** observation reports preparation failure without inventing a lifecycle for that provider or the target

#### Scenario: Capture finite task values from a streaming executable

- **WHEN** a Task selected from a stream binary requests snapshots with an available logs exporter
- **THEN** its finite snapshots use the selected-operation identity and normal bounded capture behavior

#### Scenario: Keep whole-stream snapshot restrictions

- **WHEN** the stream binary is invoked without selection and requests whole-stream snapshots
- **THEN** it retains the existing unsupported streaming capture diagnostic

#### Scenario: Preserve results during export failure

- **WHEN** an isolated Task succeeds but telemetry export is unavailable
- **THEN** the runner preserves its validated result and success status while reporting bounded export diagnostics
