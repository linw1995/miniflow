## ADDED Requirements

### Requirement: Retain instance worker settings in stream messages

Every emitted stream message context SHALL carry the instance's non-owning runtime worker handle and effective worker limit before downstream execution. The effective limit SHALL be the minimum of the runtime and stream worker limits, even when the instance pool has fewer workers because of the execution-domain layout. Downstream Loop and Iteration bodies SHALL reuse the instance pool until stream execution settles.

#### Scenario: Execute parallel Iteration after a producer emission

- **WHEN** a producer emits multiple messages into a parallel Iteration with both runtime and stream worker limits set to one
- **THEN** every message and Iteration item uses the same instance pool
- **AND** downstream execution starts no replacement worker pool

#### Scenario: Retain the smaller configured limit after an event emission

- **WHEN** a Batch event emits a message and runtime and stream worker limits differ
- **THEN** the downstream context retains the smaller configured limit and the instance pool handle

#### Scenario: Distinguish effective limits from pool capacity

- **WHEN** the execution-domain layout reduces pool capacity below the effective configured limit
- **THEN** emitted contexts retain that configured limit while using the existing smaller pool
