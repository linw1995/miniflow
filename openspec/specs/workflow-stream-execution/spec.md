# workflow-stream-execution Specification

## Purpose

Execute a workflow over an input sequence with instance-owned node state, isolated message contexts, independent timer progress, and bounded scheduling through explicit close and drain boundaries.

## Requirements

### Requirement: Isolate state by workflow instance

A streaming workflow SHALL retain its node state from instance startup through drain or termination. Separate instances of the same prepared workflow MUST have separate mutable node state and timers. Message contexts MUST remain isolated from earlier messages even when node state survives them.

#### Scenario: Accumulate across inputs

- **WHEN** several inputs reach a stateful node in one instance
- **THEN** the node can retain accepted values between inputs without recreating its state

#### Scenario: Start two instances

- **WHEN** two instances execute the same prepared workflow concurrently
- **THEN** their buffers, deadlines, message identities, and terminal outcomes are independent

#### Scenario: Release completed message context

- **WHEN** a frame has completed and no consumer references its context values
- **THEN** those values can be released while the instance remains open

#### Scenario: Reuse task and event executors

- **WHEN** one instance processes multiple messages
- **THEN** it reuses its prepared task and event nodes until the instance ends, while each message has a fresh execution context

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

### Requirement: Support event-driven emissions

Stateful execution SHALL receive input, timer, and upstream-close events and produce zero or more complete emissions per event. A successful event with no emission MUST NOT publish an empty result or conditional skip to downstream nodes. Ordinary nodes SHALL retain one invocation and one resolved result per message.

#### Scenario: Buffer without downstream execution

- **WHEN** an input updates a collector without satisfying a flush condition
- **THEN** no downstream invocation or downstream skip is scheduled for that buffered input

#### Scenario: Emit on a timer

- **WHEN** a timer event produces a batch without any new input
- **THEN** its validated emission schedules downstream processing normally

#### Scenario: Adapt an ordinary plugin

- **WHEN** an ordinary synchronous plugin receives a stream message
- **THEN** its existing execution method is invoked once with the current message's inputs and context

### Requirement: Serialize state changes and validate emissions

Events that mutate one node instance SHALL be serialized. Each operator SHALL have one replaceable deadline, cleared before a due timer callback. Event callbacks MAY read their input frame but MUST publish through returned emissions. Each emission MUST pass the existing port, value, and skip validation before any of its outputs become visible. A failed emission MUST fail the instance; earlier successfully published emissions remain published.

#### Scenario: Race input and timer delivery

- **WHEN** input and timer events become ready for one collector concurrently
- **THEN** state changes occur in a defined serial order and no element is duplicated or removed by concurrent mutation

#### Scenario: Reject an invalid emission

- **WHEN** a stateful node emits an output with the wrong declared type
- **THEN** that emission is not published and the instance reports a node publication failure

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

### Requirement: Preserve order within a message domain

A streaming instance SHALL process frames in FIFO order within each message domain. Each frame SHALL be partitioned into synchronous execution domains. Tasks within one execution domain SHALL execute serially; independent ready execution domains MAY run concurrently, and a join domain MUST wait for all predecessor domains. Distinct message domains can progress independently subject to capacity. Collected element and selected output order SHALL follow message-domain order.

#### Scenario: Process a slow earlier input

- **WHEN** an earlier frame runs a slow ordinary node before reaching a collector
- **THEN** a later frame in that message domain does not overtake it at the collector

#### Scenario: Run independent domains within one frame

- **WHEN** one frame reaches two independent ready execution domains and worker capacity is available
- **THEN** both domains can execute concurrently while retaining the same message identity

#### Scenario: Wait for both branches before a join

- **WHEN** two execution domains from one frame feed a join domain
- **THEN** the join waits for both validated branch results

#### Scenario: Continue collection during downstream work

- **WHEN** a batch-domain operation is running and upstream capacity remains available
- **THEN** the input message domain can continue filling a later frame without reordering either domain's frames

### Requirement: Drive deadlines independently of data and business work

Idle input and running ordinary plugin calls MUST NOT prevent the runtime from servicing due timers. Deadline expiry SHALL make an emission ready; actual downstream execution remains subject to capacity and earlier work in its domain.

#### Scenario: Wait for another input indefinitely

- **WHEN** the source remains open but sends nothing after one item enters collector
- **THEN** the timer can flush that item without another source read completing

#### Scenario: Run slow downstream I/O

- **WHEN** a previous batch is executing a blocking business operation
- **THEN** the collector's next due batch can be sealed while downstream execution remains queued

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

### Requirement: Bound execution per message without limiting instance age

Each message frame SHALL receive a fresh execution-step budget, with existing synchronous body budget rules retained. The budget MUST NOT accumulate across all messages in a long-lived instance. Identity and sequence counters MUST use checked arithmetic and fail before reuse or wraparound.

#### Scenario: Process more steps than a single-run limit over time

- **WHEN** an instance processes many individually valid messages whose combined work exceeds the legacy per-run step limit
- **THEN** it continues normally while each frame remains within its own budget

#### Scenario: Exhaust one message budget

- **WHEN** one frame exceeds its scheduled-step budget
- **THEN** the instance fails with that frame and node context

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

### Requirement: Run each synchronous execution domain in one dispatch

A worker SHALL execute the ordered ordinary task steps of one execution domain synchronously in one dispatch. It SHALL return domain completion to the coordinator at a fork, join, event, or stream boundary. The runtime SHALL check for instance failure between task calls and preserve the failing node identity.

#### Scenario: Execute a serial domain before a fork

- **WHEN** consecutive tasks belong to one domain and its final node forks to two branch domains
- **THEN** the worker executes the domain's tasks in order and makes both branches eligible after the domain commits

#### Scenario: Keep the coordinator available at domain boundaries

- **WHEN** a domain completes before an event node or stream producer boundary
- **THEN** the coordinator can process timers and other ready domains before dispatching that boundary

#### Scenario: Stop a domain after a failure

- **WHEN** a task fails while a domain is executing synchronously
- **THEN** the worker does not start later tasks in that domain and reports the failing node

### Requirement: Invoke event plugins without the scheduler lock

The runtime SHALL invoke EventNode callbacks, buffered inspection, and terminal reporting without the scheduler mutex while preserving serial ownership. Failure or cancellation recorded before publication SHALL discard emissions and timer updates. After reacquiring the mutex, the coordinator SHALL check failure before propagating dependency errors or scheduling, including cancellation recorded before its waker acquires the mutex. The first recorded failure SHALL remain the terminal cause.

#### Scenario: Cancel from an input callback

- **WHEN** an EventNode input callback cancels its workflow and then returns emissions or timer updates
- **THEN** cancellation returns without lock reentry deadlock
- **AND** those effects are not committed and the instance retains the cancellation failure

#### Scenario: Cancel during buffered-state inspection

- **WHEN** a node cancels its workflow while the runtime reads its buffered-item count
- **THEN** inspection completes without holding the scheduler mutex
- **AND** the returned event effects are discarded

#### Scenario: Query and cancel during a running callback

- **WHEN** an event callback is waiting and another thread queries instance state and cancels the workflow
- **THEN** state queries and cancellation complete before the callback returns
- **AND** cleanup waits for the callback to settle without publishing its returned effects

#### Scenario: Cancel while reporting a completed event

- **WHEN** a synchronous observer cancels the workflow while the runtime reports a committed event
- **THEN** reporting completes without scheduler lock reentry
- **AND** no subsequent event callback or selected output is scheduled after that failure

#### Scenario: Cancel while reporting a dependency failure

- **WHEN** a synchronous observer cancels the workflow while the runtime reports a stream operator dependency failure
- **THEN** reporting completes without scheduler lock reentry
- **AND** cancellation recorded before the scheduler reacquires the mutex remains the terminal cause

#### Scenario: Cancel while reporting a skipped operator

- **WHEN** a synchronous observer cancels the workflow while the runtime reports a skipped stream operator
- **THEN** reporting completes without scheduler lock reentry
- **AND** the coordinator does not complete the current frame, invoke further nodes, or publish selected outputs
