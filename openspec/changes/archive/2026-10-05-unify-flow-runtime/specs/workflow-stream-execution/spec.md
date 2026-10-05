# workflow-stream-execution Specification

## MODIFIED Requirements

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

## REMOVED Requirements

### Requirement: Run consecutive synchronous tasks in one dispatch

**Reason**: Task dispatch is now defined by serial execution domains. A message domain may contain multiple execution domains, and independent domains can progress concurrently.

**Migration**: Use the execution-domain dispatch contract, which retains synchronous execution within each ordered domain and adds concurrent scheduling between ready domains.

#### Scenario: Execute a chain before an event boundary

- **WHEN** a frame reaches consecutive ordinary tasks followed by an event node
- **THEN** one worker dispatch executes those tasks in topological order and returns the frame before delivering the event
- **AND** the coordinator remains available for timers in other domains

#### Scenario: Observe failure between consecutive calls

- **WHEN** an instance failure is recorded while a task in the current dispatch is running
- **THEN** that task finishes normally and the worker does not start the following task

## ADDED Requirements

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
