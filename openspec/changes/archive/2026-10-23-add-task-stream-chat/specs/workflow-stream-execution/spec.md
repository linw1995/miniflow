## MODIFIED Requirements

### Requirement: Preserve message identity through ordinary dependencies

Ordinary nodes SHALL resolve dependencies within one message domain and identity, preserved by fan-out. Initial tasks SHALL share one startup frame. Collector and producer emissions, including combined-node emissions, SHALL create new domains; combined terminal outputs SHALL retain the input domain. Startup values MUST NOT be implicitly broadcast. Cross-domain dependencies MUST be rejected unless a supported explicit correlation operation defines their relationship.

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

#### Scenario: Join ordinary startup values

- **WHEN** two initial tasks produce values consumed by one ordinary task before any stream boundary
- **THEN** that task receives both values in the same startup frame and executes once

#### Scenario: Reject a startup value mixed with a stream message

- **WHEN** a downstream task combines a startup constant with the output of an independent producer
- **THEN** validation rejects the domain mismatch rather than broadcasting or retaining the constant as global state

#### Scenario: Keep a combined answer in its input frame

- **WHEN** a combined node emits deltas and returns a terminal answer
- **THEN** downstream delta consumers receive emission-domain messages and terminal consumers receive the answer in the input domain

#### Scenario: Reject a delta joined with its final answer

- **WHEN** one consumer binds a delta and terminal answer from the same combined node
- **THEN** preparation reports the cross-domain dependency rather than correlating them by arrival order

## ADDED Requirements

### Requirement: Preserve combined invocation closure and drain

A combined invocation SHALL finish production when its execution entry point returns or fails. Admitted emissions SHALL retain their delivery obligations. A successful zero-emission invocation SHALL release its input frame's Task dependencies without creating an emission frame. Closing one invocation MUST NOT close the operator before later upstream invocations finish.

#### Scenario: Handle an empty invocation before a nonempty invocation

- **WHEN** a combined node emits nothing for one input and emits values for the next
- **THEN** the first Task result completes normally and later emissions remain deliverable

#### Scenario: Drain after terminal publication

- **WHEN** terminal output has been published and an earlier delta still has pending downstream work
- **THEN** the instance cannot report successful completion until that connected work drains

### Requirement: Keep selected-output domain rules explicit

Existing selected stream-workflow results SHALL retain their single-message-domain restriction. Explicit live exports SHALL be independently routed and MUST NOT merge terminal and emission domains into one synthetic result. Ownership validation SHALL use source output category rather than one domain per node.

#### Scenario: Select terminal output and export deltas

- **WHEN** a streaming graph selects an origin-domain terminal output and separately exports a combined stream
- **THEN** both routes retain their own identities without permitting a mixed-domain selected output object
