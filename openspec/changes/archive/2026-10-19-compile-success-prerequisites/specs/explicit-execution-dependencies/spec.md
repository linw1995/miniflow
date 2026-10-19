## RENAMED Requirements

- FROM: `### Requirement: Maintain authoritative bounded invocation outcomes`
- TO: `### Requirement: Compile success activation without runtime outcomes`

## MODIFIED Requirements

### Requirement: Resolve node success independently of outputs

A node-success prerequisite SHALL be satisfied only by successful operation and normal output validation/publication in
the current frame. A skipped source SHALL skip its consumer. The compiler SHALL derive source activation from its
input/output gates and transitive success prerequisites. Validated scheduling SHALL wait for predecessor completion;
source errors and cancellation SHALL terminate execution without activating dependents.

#### Scenario: Complete a zero-output Task

- **WHEN** a Task with no declared outputs successfully completes
- **THEN** its node-success consumers become eligible subject to all their other prerequisites

#### Scenario: Skip every optional output after success

- **WHEN** a Task succeeds while explicitly skipping all optional outputs
- **THEN** its node-success edge is satisfied while its output gates remain skipped

#### Scenario: Reject premature success

- **WHEN** a source operation returns but its outputs fail validation
- **THEN** its node-success consumers do not run

#### Scenario: Preserve branch skips

- **WHEN** a branch gate skips a Task with a node-success dependent
- **THEN** the dependent also skips instead of bypassing the branch condition

### Requirement: Preserve conjunctive scheduling and errors

Data inputs, output gates, and node-success prerequisites SHALL all participate in ordering, cycles, ancestry, and
readiness. Target activation SHALL require all logical prerequisites. Existing dependency-error precedence over skips
SHALL remain. Redundant execution and data dependencies MAY coexist; ordering SHALL deduplicate node pairs without
dropping semantic requirements.

#### Scenario: Require both successful predecessors

- **WHEN** a Task has two node-success predecessors and one remains running
- **THEN** it cannot start based on the other predecessor's success

#### Scenario: Detect a mixed dependency cycle

- **WHEN** input references and node-success edges together form a cycle
- **THEN** compilation rejects it

#### Scenario: Do not hide missing state behind a skip

- **WHEN** a scheduled consumer has an missing required gate output and another prerequisite is skipped
- **THEN** resolution fails before invoking it rather than silently skipping

### Requirement: Compile success activation without runtime outcomes

In-memory preparation SHALL bind success predicates, and generated runners SHALL emit direct Rust conditions. Runtime
execution SHALL NOT interpret node-success dependency records or retain a Task outcome map. Predicates SHALL use
current-frame output availability, preserve exclusive alternatives, and report the immediate predecessor as a skip
cause. Unconditional predecessors SHALL require ordering without an output probe.

#### Scenario: Run without observation

- **WHEN** snapshots and telemetry are disabled
- **THEN** zero-output success prerequisites resolve through validated scheduling and compiled activation conditions

#### Scenario: Process another message

- **WHEN** a later stream message skips a Task that ran on an earlier message
- **THEN** its dependent skips based on current-frame gates without consulting prior success state

#### Scenario: Preserve exclusive alternatives

- **WHEN** a source executes with one produced exclusive input candidate and other candidates skipped
- **THEN** its success consumer remains active rather than requiring every candidate to be produced

#### Scenario: Execute a generated optimized Task

- **WHEN** generated execution uses typed data paths or falls back for snapshots
- **THEN** both paths apply the compiled success condition and preserve the publication boundary

#### Scenario: Reject changed success contracts during generated preparation

- **WHEN** a provider no longer permits ordinary success prerequisites when a generated executable prepares its nodes
- **THEN** preparation rejects the graph before invoking any node

### Requirement: Bound success edges to ordinary same-frame Tasks

Node-success edges SHALL require ordinary Task endpoints in one scope and input message frame. They MAY connect
supported body Tasks or Tasks in one stream-message domain. Event/Stream operators, synthetic sources, Loop/Iteration
containers, and cross-scope/domain endpoints MUST be rejected. These restrictions SHALL NOT narrow existing data or
output-gate support.

#### Scenario: Order body Tasks

- **WHEN** two ordinary Tasks within a supported body use a success edge
- **THEN** each body invocation evaluates only its own predecessor activation conditions

#### Scenario: Reject operator lifetime ordering

- **WHEN** a portless edge names a Stream operator or a container endpoint
- **THEN** preparation rejects it rather than interpreting a message or body step as whole-node completion
