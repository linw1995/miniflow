## ADDED Requirements

### Requirement: Reuse byte measurements

Byte accounting SHALL reuse cached Rust heap estimates across shared immutable values and SHALL
update retained context totals from changed bindings. It MUST preserve
per-binding charges and reject every over-budget publication before exposing any of its outputs.

#### Scenario: Reuse a value with a different limit

- **WHEN** a shared value is measured under one limit and then checked under another
- **THEN** the cached heap estimate is compared against the new limit without traversing the value again

#### Scenario: Retain a vector with spare capacity

- **WHEN** a retained array has more allocated slots than elements
- **THEN** its memory charge includes that capacity and the cached heap estimates of its elements
- **AND** forwarding a shared value reuses its estimate without traversing its children

#### Scenario: Retain shared children

- **WHEN** several bindings or container entries reference the same value
- **THEN** each retaining reference is charged the value's cached heap estimate

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

## MODIFIED Requirements

### Requirement: Configure streaming byte budgets

Streaming execution SHALL accept positive `max_record_bytes`, `max_message_bytes`, and
`max_buffered_bytes` settings, defaulting to 1 MiB, 1 MiB, and 64 MiB respectively. Record limits
measure JSON bytes; message and instance budgets measure estimated retained Rust memory. Workflow
and compiled-plan JSON entry points and execution preparation MUST reject incompatible memory
budgets before admitting input.

#### Scenario: Use defaults

- **WHEN** streaming limits omit the byte fields
- **THEN** the instance uses a 1 MiB record limit, 1 MiB message memory limit, and 64 MiB retained-memory budget

#### Scenario: Reject an impossible budget

- **WHEN** the memory budget cannot cover the graph's frame, callback, flush, and input reserves
- **THEN** preparation fails with a byte-budget diagnostic before any input is accepted

### Requirement: Account for estimated retained memory

The runtime SHALL account for queued inputs, active message contexts, retained event values, pending
emissions, and selected outputs using cached Rust heap estimates and metadata allowances. Estimates
SHALL include value allocations, string storage, vector capacity, estimated tree entries, and child
values. Shared allocations SHALL be charged per retaining reference. Event providers MUST report
estimated retained heap bytes through `retained_bytes`, excluding returned emissions. These budgets
do not represent process RSS or arbitrary unreported allocations made by plugins.

#### Scenario: Buffer several inputs

- **WHEN** an event node retains values across input callbacks
- **THEN** its estimated heap use participates in the instance's retained-memory budget

#### Scenario: Emit a collected buffer

- **WHEN** Batch seals its retained items
- **THEN** its vector capacity and retained values transfer to the pending emission's memory charge

### Requirement: Bound JSON Lines transport buffers

Generated streaming runners SHALL enforce `max_record_bytes` during input framing and output
serialization independently of memory budgets. Oversized records MUST identify the input line.
Output failure MUST be reported before successful completion.

#### Scenario: Reject an oversized record before parsing it

- **WHEN** an input line grows beyond `max_record_bytes`
- **THEN** the runner stops that record with a line-numbered size diagnostic

## RENAMED Requirements

- FROM: `### Requirement: Account for retained logical values`
- TO: `### Requirement: Account for estimated retained memory`
