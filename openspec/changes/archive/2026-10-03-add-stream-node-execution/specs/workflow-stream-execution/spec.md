## MODIFIED Requirements

### Requirement: Preserve message identity through ordinary dependencies

Ordinary nodes SHALL resolve all data and control dependencies for one message identity within one message domain. Fan-out SHALL preserve that identity. Nodes that collect inputs or produce incremental results SHALL establish a new domain. Dependencies from different domains MUST be rejected unless an explicit supported correlation operation defines their relationship.

#### Scenario: Rejoin two branches of one input

- **WHEN** two ordinary branches of the same input feed a consumer
- **THEN** the consumer receives values from that same input and retains the existing conjunctive skip behavior

#### Scenario: Reject item and batch mixing

- **WHEN** a consumer binds one input from before collector and another from its emitted batch
- **THEN** validation rejects the mixed message domains with the affected endpoints

#### Scenario: Reject independently formed batch joins

- **WHEN** a consumer joins outputs from two distinct collector nodes with equal thresholds
- **THEN** validation rejects the join rather than pairing batches by arrival order or local sequence number

#### Scenario: Process incremental outputs independently

- **WHEN** a producer emits while its input invocation is still running
- **THEN** the new output domain can progress independently while preserving output order

### Requirement: Stop scheduling on failure

Failure SHALL stop admission and new runtime scheduling, clear deadlines, discard pending work and buffers, and suppress later workflow outputs from already running work. Failure cleanup MUST NOT flush a tail or retry automatically. Previously delivered results and side effects remain effective. Blocked producer sends SHALL wake and fail. Already started task and producer calls, including synchronous bodies, run to completion before the instance releases their executors.

#### Scenario: Fail after an earlier batch succeeded

- **WHEN** a later batch operation fails after an earlier result was delivered
- **THEN** the instance fails, retains the delivered prefix, and neither replays that prefix nor reports rollback

#### Scenario: Receive a late worker result after failure

- **WHEN** already running work returns after the instance fails
- **THEN** its result does not schedule new downstream work or produce a new selected output

#### Scenario: Fail a consumer while production is blocked

- **WHEN** a downstream task fails while an upstream producer waits for queue capacity
- **THEN** the producer send fails and cleanup waits for that invocation to settle
