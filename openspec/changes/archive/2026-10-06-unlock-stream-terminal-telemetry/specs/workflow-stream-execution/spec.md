## MODIFIED Requirements

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
