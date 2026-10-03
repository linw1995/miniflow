## ADDED Requirements

### Requirement: Reuse exact byte measurements

Byte accounting SHALL reuse successful compact JSON length measurements across shared immutable
values and SHALL update retained context totals from changed bindings. It MUST preserve logical
per-binding charges and reject every over-budget publication before exposing any of its outputs.
Bounded measurements that stop early MUST NOT be reused as exact lengths.

#### Scenario: Reuse a value with a different limit

- **WHEN** a shared value is measured under one limit and then checked under another
- **THEN** an exact cached length is compared against the new limit
- **AND** an earlier over-limit attempt does not prevent a later valid measurement

#### Scenario: Replace or skip a retained binding

- **WHEN** a publication replaces an existing value or marks its binding skipped
- **THEN** the context updates only the affected charges
- **AND** failed validation leaves both prior values and byte totals unchanged

#### Scenario: Leave a nested execution scope

- **WHEN** a nested scope finishes, fails, or unwinds
- **THEN** the parent output map and its byte accounting are restored together

### Requirement: Encode each output record once

The transport SHALL serialize each output record once into a bounded buffer. It MUST reject a
record that exceeds its limit before writing any of that record to the output descriptor.

#### Scenario: Encode a record at its limit

- **WHEN** the serialized output fits exactly within the configured limit
- **THEN** the complete record is delivered and acknowledged normally

#### Scenario: Reject an oversized encoded record

- **WHEN** encoding would exceed the configured limit
- **THEN** the instance reports a resource failure without publishing a partial record
