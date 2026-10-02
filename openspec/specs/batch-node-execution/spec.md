# batch-node-execution Specification

## Purpose

Collect ordered stream values into explicitly typed batches and emit them on count, elapsed waiting time, or upstream completion so downstream nodes operate once per whole batch.

## Requirements

### Requirement: Provide an explicitly selected Batch node

`mfn-core` SHALL register `builtin.batch` with required input `item` and required output `items`. Workflows MUST explicitly select a package registering the kind. Each emitted array SHALL be one downstream message, with no implicit per-element execution or result redistribution.

#### Scenario: Execute a business node once per batch

- **WHEN** Batch emits `[1, 2, 3]` to an ordinary node
- **THEN** the node executes once with the complete array

#### Scenario: Omit the provider dependency

- **WHEN** a workflow references `builtin.batch` without selecting a package that registers it
- **THEN** validation reports the unavailable kind before executable installation

### Requirement: Validate explicit count and wait limits

Batch configuration SHALL require positive integer `max_items` and `max_wait_ms`. Unknown fields, missing fields, zero, fractional or negative values, and values that cannot be represented safely by the runtime MUST fail validation. Both thresholds SHALL be active for each nonempty buffer.

#### Scenario: Configure immediate single-item batches

- **WHEN** configuration uses `max_items: 1` and a valid positive wait limit
- **THEN** each accepted item immediately produces one single-element batch

#### Scenario: Reject a disabled or overflowing deadline

- **WHEN** `max_wait_ms` is absent, zero, fractional, or too large for safe deadline arithmetic
- **THEN** validation fails with the Batch node and configuration field

### Requirement: Preserve element type and value identity

Batch SHALL derive output `List(T)` from its bound input type `T` and validate emitted values against that resolved type. It MUST preserve element order, null values, and nested arrays without coercion or flattening. Collection MUST NOT imply a compile-time exact value for the whole batch.

#### Scenario: Infer integer batches

- **WHEN** input `item` is bound to `Int64`
- **THEN** output `items` has type `List(Int64)`

#### Scenario: Collect arrays as items

- **WHEN** Batch receives `[1, 2]` and `[3]` as two input values
- **THEN** their batch is `[[1, 2], [3]]`

#### Scenario: Preserve null as an element

- **WHEN** a valid input value is JSON null
- **THEN** it occupies an ordinary element position and is not treated as absence or a skip

### Requirement: Flush on reaching the count threshold

Before its deadline, a buffer SHALL be sealed as soon as its count reaches `max_items`. Every emitted batch MUST contain at most `max_items` elements. Remaining arrivals SHALL enter a new buffer.

#### Scenario: Reach the configured size exactly

- **WHEN** `max_items` is three and three values arrive before the deadline
- **THEN** Batch emits those three values without waiting for a fourth value or a timer

#### Scenario: Continue after a full batch

- **WHEN** a fourth value arrives after a three-element batch is sealed
- **THEN** it begins the next buffer with a new deadline

### Requirement: Measure timeout from the first buffered item

The first item in an empty buffer SHALL start a monotonic deadline at its acceptance time plus `max_wait_ms`. Later arrivals MUST NOT extend that deadline. At or after the deadline, a nonempty buffer SHALL be sealed even if the input source remains idle and open.

#### Scenario: Flush a lone item

- **WHEN** one item arrives and no further item arrives before the wait limit
- **THEN** the timer emits a single-element batch while input remains open

#### Scenario: Avoid debounce behavior

- **WHEN** items keep arriving below the count threshold shortly before the deadline
- **THEN** the original first-item deadline still seals the buffer

### Requirement: Resolve deadline races without duplicate batches

An input accepted at or after the existing buffer's deadline SHALL first seal that buffer for timeout and then enter a new buffer. A sealed buffer MUST emit only once; cancelled or replaced deadlines MUST NOT flush a later buffer.

#### Scenario: Arrive exactly at the deadline

- **WHEN** a third value arrives at the deadline of a two-element buffer configured for three elements
- **THEN** the old two-element buffer flushes for timeout and the third value begins a new buffer

#### Scenario: Pass a cancelled deadline

- **WHEN** a count-triggered batch has already emitted and time passes its cancelled deadline after a new buffer starts
- **THEN** the new buffer and its deadline remain unchanged

#### Scenario: Receive timeout and close together

- **WHEN** the final buffer is due and upstream closure is processed
- **THEN** it emits once and no later timer emits it again

### Requirement: Flush the final partial buffer on upstream completion

After all prior upstream messages are resolved, Batch SHALL seal a nonempty final buffer before closing its output sequence. Empty buffers MUST NOT emit on timeout, close, or repeated close notification.

#### Scenario: Flush a finite input tail

- **WHEN** five values arrive before deadlines with `max_items: 3` and upstream then closes
- **THEN** downstream receives a three-element batch followed by a two-element tail

#### Scenario: Close after an exact multiple

- **WHEN** the final input already completed a full batch
- **THEN** upstream close produces no additional empty array

### Requirement: Preserve sealed batches under backpressure

A sealed batch SHALL retain its assigned membership, order, and output identity while waiting for downstream capacity. Later input MUST NOT be appended to it. Both sealed and accumulating buffers SHALL participate in runtime limits, and pressure MUST propagate upstream before retention becomes unbounded.

#### Scenario: Wait behind a slow consumer

- **WHEN** a batch is sealed while the downstream domain is busy
- **THEN** the batch remains unchanged until delivery, and later input is either buffered separately within limits or backpressured

#### Scenario: Exceed the aggregate payload limit

- **WHEN** collected elements would produce an array larger than the configured message limit
- **THEN** publication fails with the Batch node identity instead of emitting an oversized array or inventing another flush trigger

### Requirement: Exclude skipped inputs from accumulation

A Batch input with a resolved skipped data or control dependency SHALL contribute no item and MUST NOT start or reset a deadline. It MUST NOT turn an existing buffer into a skipped batch. Unexpectedly missing dependencies SHALL remain errors under ordinary dependency rules.

#### Scenario: Skip between two accepted inputs

- **WHEN** a conditional branch skips an input between two accepted values
- **THEN** the buffer contains only the accepted values in order and retains its original deadline

#### Scenario: Skip every input

- **WHEN** all inputs reaching a Batch boundary are skipped
- **THEN** no batch is emitted and upstream closure completes normally
