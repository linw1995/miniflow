# Spec Delta

## MODIFIED Requirements

### Requirement: Distinguish admission from processing completion

Admission through an explicit host-fed source SHALL complete when that source accepts ownership of the input.
Each admission and close handle SHALL address one source; the workflow MUST NOT require an instance-wide input
source. It MUST NOT wait for that input to produce a batch or finish downstream processing. Output delivery
SHALL be independently consumable. The API MUST distinguish capacity pressure, closed admission, invalid
input, and terminal failure.

#### Scenario: Submit enough items to form a batch

- **WHEN** a caller submits inputs sequentially while concurrently consuming outputs
- **THEN** accepted sends can complete before the first batch exists, allowing later inputs to reach the
  collector

#### Scenario: Close before another send

- **WHEN** the caller closes input and then submits another value
- **THEN** the later send fails as closed without accepting that value

### Requirement: Preserve message identity through ordinary dependencies

Ordinary nodes SHALL resolve data and control dependencies within one message domain and identity, preserved
by fan-out. Initial tasks and ordinary startup dependencies SHALL share one startup frame. Collectors and
producers SHALL create new domains; startup values MUST NOT be implicitly broadcast across them. Cross-domain
dependencies MUST be rejected unless an explicit supported correlation operation defines their relationship.

#### Scenario: Rejoin two branches of one input

- **WHEN** two ordinary branches of the same input feed a consumer
- **THEN** the consumer receives values from that same input and retains the existing conjunctive skip
  behavior

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
- **THEN** validation rejects the domain mismatch rather than broadcasting or retaining the constant as global
  state

### Requirement: Bound pending messages and worker concurrency

Instances SHALL enforce finite limits on source admission, per-operator pending emissions, active frames, and
worker concurrency. Source admission and emission SHALL apply backpressure. Startup and each downstream
message domain SHALL have reserved progress capacity so producers cannot consume every permit needed by their
consumers. These count limits do not bound payload size or data retained inside a plugin.

#### Scenario: Stop consuming workflow output

- **WHEN** an output consumer stops reading while inputs continue
- **THEN** output pressure eventually reaches input admission without unbounded queues or discarded results

### Requirement: Preserve progress when admission is full

Capacity limits SHALL reserve progress for timer handling, closure, worker completion, and downstream handoff.
Configurations with fewer pending-message slots than the startup frame plus producer/event output domains MUST
fail before execution. Producer startup, a flush, and source closure MUST NOT require another external input
permit. Pending emissions MUST stay within each operator's configured message-count limit until handed off.

#### Scenario: Use a batch count larger than input capacity

- **WHEN** a valid bounded instance has full input admission, a partial batch, and a collection threshold
  larger than the available input-frame capacity
- **THEN** its deadline can seal and deliver the batch, permitting progress without another input

#### Scenario: Reject an unusable capacity configuration

- **WHEN** max_pending_messages is smaller than the required startup and message-domain reservations
- **THEN** startup fails with a limit diagnostic before accepting input

### Requirement: Drain after closing input

Closing an explicit host source SHALL be idempotent and stop admission to that source. Each source SHALL
finish admitted work before closing dependent nodes. Success MUST wait for startup traversal, every source and
domain to close, all started work to settle, timers to be cleared, and all selected results to reach the
configured output sink. No host input handle SHALL be required for autonomous completion.

#### Scenario: Close while upstream transformation is running

- **WHEN** the host closes one source while an admitted frame is still computing a collector input
- **THEN** the computed value reaches collector before its upstream-close event and is included in its full or
  tail batch

#### Scenario: Keep a final output pending

- **WHEN** every node has finished but a selected result is still waiting for sink capacity
- **THEN** the instance remains draining and cannot report successful completion

#### Scenario: Keep an independent source alive

- **WHEN** one source ends while another source can still emit
- **THEN** the first branch drains independently and the workflow remains active

#### Scenario: Complete without external input

- **WHEN** all autonomous producers return and their selected results have been delivered
- **THEN** the workflow finishes without waiting for stdin EOF or a host close call

### Requirement: Propagate closure through emitted messages

An operator's output sequence SHALL close only after all its emissions are delivered and no future input or
timer can emit. Downstream closure MUST wait for every contributing upstream sequence and prior message. Close
processing SHALL reach downstream collectors even when upstream emitted no data.

#### Scenario: Drain chained collectors

- **WHEN** input closes with partial buffers in two collector nodes connected in sequence
- **THEN** the first tail reaches the second before the second closes, and all resulting downstream work
  completes

#### Scenario: Close an empty stream

- **WHEN** a started source ends without emitting any values
- **THEN** closure reaches its downstream domains, its message consumers remain uninvoked, and no empty batch
  is produced

#### Scenario: Close a collector behind an inactive branch

- **WHEN** every upstream frame skips the branch feeding a collector
- **THEN** that collector still receives closure and the instance can finish without inventing downstream
  messages

## ADDED Requirements

### Requirement: Activate the workflow once at startup

After validating all workflow parameters and resources, streaming execution SHALL activate initial tasks and
stream producers once per instance without injecting `%input` or admitting an external trigger. Ordinary
startup dependencies SHALL retain deterministic ordering and conditional semantics. Startup activation MUST
NOT recur when a producer emits.

#### Scenario: Run a task-only streaming workflow

- **WHEN** a stream-mode graph contains only initial tasks and ordinary task dependencies
- **THEN** it executes once, delivers its selected result once, and completes without stdin

#### Scenario: Run an empty workflow

- **WHEN** a valid stream-mode graph has no nodes or selected outputs
- **THEN** it completes without waiting for an external input resource

#### Scenario: Skip a producer during startup

- **WHEN** a startup task skips the control output activating a producer
- **THEN** the producer is not invoked and its downstream domain closes without a fabricated emission
