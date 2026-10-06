## ADDED Requirements

### Requirement: Invoke event plugins without the scheduler lock

The runtime SHALL invoke EventNode callbacks and buffered inspection without the shared scheduler mutex while preserving serial ownership. Failure or cancellation recorded before effects publication SHALL discard returned emissions and timer updates. Reporting a completed event SHALL allow synchronous cancellation, and the coordinator SHALL recheck failure before further scheduling. The first recorded failure SHALL remain the terminal cause.

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
