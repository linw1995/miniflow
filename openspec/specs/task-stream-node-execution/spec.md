# task-stream-node-execution Specification

## Purpose

Run one operation that emits optional incremental values and returns an authoritative terminal Task result.

## Requirements

### Requirement: Invoke combined execution once

A combined executor SHALL receive resolved inputs, a mutable execution context, and a borrowed emitter
through one business entry point. It SHALL return one terminal `NodeResult` and MAY emit zero or more
incremental results before returning. The runtime MUST NOT invoke separate Task and Stream business
methods or reconstruct the terminal result from emitted values.

#### Scenario: Emit before returning the final result

- **WHEN** a combined operation emits several values and then returns its final result
- **THEN** a connected sink receives those values before the operation returns and business logic runs once

#### Scenario: Return without emitting

- **WHEN** a combined operation returns successfully without any emission
- **THEN** its terminal result remains available and no incremental result is invented

### Requirement: Reflect separate output contracts

Typed combined providers SHALL declare associated input, terminal output, and emission value contracts.
Dynamic providers SHALL reflect their input and terminal ports through `NodePortContract` and their
incremental ports through its stream-output declaration. Preparation MUST NOT execute business logic.
Terminal and incremental port names MUST be distinct, incremental names MUST be unique, and competing
metadata declarations MUST fail during preparation.

#### Scenario: Reject an ambiguous output category

- **WHEN** terminal and incremental contracts use the same port name
- **THEN** preparation fails with the conflicting port identity

#### Scenario: Reject competing declarations

- **WHEN** caller-supplied metadata redeclares reflected incremental ports
- **THEN** preparation fails before an executor is returned

### Requirement: Validate every attempted emission

The emitter SHALL encode typed results and validate each emitted result against its incremental contract.
Required fields SHALL constrain each emitted record rather than require an invocation to emit. Encoding,
validation, and delivery failures MUST retain their typed source chains and suppress terminal success
even if provider code ignores the immediate send error. Cancellation SHALL be checked before and after
business execution and on attempted publication.

#### Scenario: Ignore a send failure

- **WHEN** a provider ignores an encoding or delivery failure and returns a successful terminal result
- **THEN** combined execution still fails with the original typed publication cause

#### Scenario: Emit an invalid discarded record

- **WHEN** an unconnected provider attempts to emit a value with missing or incompatible fields
- **THEN** emission validation fails despite the absence of a consumer

### Requirement: Adapt terminal-only execution without a receiver

Combined executors SHALL support ordinary Task borrowing and ownership conversion. Terminal-only
adaptation SHALL use an unconnected discard sink without allocating an unread receiver. Providers MAY
query connectivity and omit incremental construction. The adapter SHALL preserve the provider's declared
success-dependency capability. Finite and streaming workflows SHALL support this terminal Task path;
graph-local incremental routing and host export are separate planning capabilities.

#### Scenario: Execute repeated terminal-only calls

- **WHEN** a combined executor is invoked repeatedly through its Task adapter
- **THEN** every invocation returns its own terminal result without waiting for incremental consumption

#### Scenario: Preserve generated and isolated execution

- **WHEN** a generated finite or streaming runner executes a combined node normally or as an isolated target
- **THEN** it publishes the same terminal outputs through the existing Task contract

### Requirement: Observe emissions without changing business values

Combined execution SHALL report successfully admitted emissions through an attached snapshot recorder.
Snapshot capture SHALL use the existing redaction policy and MUST NOT redact the values delivered to
the business sink. Business delivery SHALL remain available without an attached recorder.

#### Scenario: Capture a sensitive incremental value

- **WHEN** a combined executor sends a secret-marked value with snapshot capture enabled
- **THEN** the sink receives the original value while the snapshot contains its redacted representation
