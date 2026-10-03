# workflow-stream-byte-budgets Specification

## Purpose

Bound streaming payloads and retained logical values through byte accounting and progress reserves,
independently of message-count scheduling, node lifetimes, and Batch collection policy.

## Requirements

### Requirement: Configure streaming byte budgets

Streaming execution SHALL accept positive `max_message_bytes` and `max_buffered_bytes` settings,
defaulting to 1 MiB and 64 MiB respectively. Workflow and compiled-plan JSON entry points and execution
preparation MUST reject incompatible values before admitting input.

#### Scenario: Use defaults

- **WHEN** streaming limits omit the byte fields
- **THEN** the instance uses a 1 MiB payload limit and a 64 MiB logical retained-value budget

#### Scenario: Reject an impossible budget

- **WHEN** the byte budget cannot cover the graph's frame, callback, flush, and input reserves
- **THEN** preparation fails with a byte-budget diagnostic before any input is accepted

### Requirement: Account for retained logical values

The runtime SHALL account for queued inputs, active message contexts, retained event values, pending
emissions, and selected outputs using encoded value sizes and metadata allowances. Event providers
MUST report retained logical values through `retained_bytes`, excluding returned emissions. These
budgets do not represent process RSS or arbitrary allocations made by plugins.

#### Scenario: Buffer several inputs

- **WHEN** an event node retains values across input callbacks
- **THEN** its reported retained bytes participate in the instance's logical byte budget

#### Scenario: Emit a collected buffer

- **WHEN** Batch seals its retained items
- **THEN** retained-state accounting transfers to the pending emission and downstream frame without losing the budget charge

### Requirement: Backpressure admission by bytes

Input admission SHALL require both count capacity and byte capacity. `send` MUST wait when byte
capacity is temporarily unavailable; `try_send` MUST return `Capacity` without accepting the value.
Runtime progress and output acknowledgement SHALL wake waiting producers to recheck admission.

#### Scenario: Reach byte capacity before the frame limit

- **WHEN** outputs remain pending and queued values fill the available byte allowance
- **THEN** admission reports capacity pressure even when the configured frame count has not been reached

#### Scenario: Resume after downstream progress

- **WHEN** output acknowledgement lets retained work advance and sufficient byte allowance becomes available
- **THEN** waiting producers can admit subsequent values

### Requirement: Reserve byte capacity for progress

The runtime SHALL reserve byte capacity for message frames, event-input handoff, and sealed emissions.
Source admission MUST leave these reserves available. Timer and close emissions MUST NOT require a new
external input permit, and pending emissions MUST remain charged until handed off.

#### Scenario: Flush with full admission

- **WHEN** admitted inputs occupy the source allowance and a collector deadline expires
- **THEN** the collector can seal its buffer using reserved capacity and schedule downstream work subject to domain availability

### Requirement: Fail oversized publication explicitly

An input or node result exceeding the payload limit MUST fail before admission or publication.
A message context or reported event state exceeding its reservation or available budget MUST fail
explicitly. Failure cleanup SHALL retain the parent runtime's ordering and delivered-prefix behavior.

#### Scenario: Reject oversized input

- **WHEN** a submitted value exceeds `max_message_bytes`
- **THEN** it is not admitted and the instance reports a resource failure

#### Scenario: Reject oversized collected output

- **WHEN** individually valid items produce a batch exceeding the payload limit
- **THEN** the batch is not published and the producing node is identified in the failure

#### Scenario: Reject excessive retained context

- **WHEN** a task adds more output or context data than its frame reservation permits
- **THEN** publication fails before downstream work observes those values

### Requirement: Bound JSON Lines transport buffers

Generated streaming runners SHALL enforce record and encoded-payload limits during input framing and
bound output serialization using the delivery reservation. Oversized records MUST identify the input
line. Output failure MUST be reported before successful completion.

#### Scenario: Reject an oversized record before parsing it

- **WHEN** an input line grows beyond `max_message_bytes`
- **THEN** the runner stops that record with a line-numbered size diagnostic

### Requirement: Observe byte-budget failure

Stream observation SHALL report byte-limit publication failures as failures and MUST NOT report the
rejected publication as successful. Terminal observation SHALL follow the normal failure cleanup.

#### Scenario: Reject a task's oversized result

- **WHEN** byte validation rejects a task result
- **THEN** that node and the stream report failure and no successful publication is emitted for the result
