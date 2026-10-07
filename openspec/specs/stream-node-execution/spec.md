# stream-node-execution Specification

## Purpose

Execute input-driven producers that emit results incrementally while respecting downstream capacity, ordered completion, and workflow cancellation.

## Requirements

### Requirement: Emit incrementally for each input

A stream producer SHALL receive its startup parameters once when initially activated, or one resolved input
per upstream invocation, together with its execution context. It SHALL emit zero or more complete results
before returning. Results SHALL become eligible for downstream processing before the invocation returns. The
runtime MUST NOT require collection of the full output sequence.

#### Scenario: Produce more results than queue capacity

- **WHEN** one input generates thousands of results with a pending-message limit of two
- **THEN** downstream processing receives every result in order without a capacity failure

#### Scenario: Emit nothing

- **WHEN** a producer returns successfully without sending a result
- **THEN** no downstream invocation or skip is invented and later inputs can proceed

#### Scenario: Start a parameterized source without an upstream message

- **WHEN** an initial producer receives valid workflow startup arguments
- **THEN** it executes once and can emit incrementally without stdin or a synthetic trigger message

### Requirement: Block sends under downstream pressure

Each send SHALL transfer one validated result into a bounded per-node queue. A full queue SHALL suspend the sender until capacity becomes available or the instance fails. Successful sending SHALL mean admission rather than downstream completion. Blocked producers MUST NOT prevent downstream tasks or timers from running.

#### Scenario: Stop consuming with one task worker

- **WHEN** a producer fills its output queue while the consumer pauses and only one ordinary task worker is configured
- **THEN** sends wait within the configured bound and resume in order when consumption continues

#### Scenario: Preserve timer progress

- **WHEN** a producer is executing or waiting for output capacity while a collector deadline becomes due
- **THEN** the runtime can deliver the timer independently

### Requirement: Serialize and isolate producer state

Each workflow instance SHALL own independent mutable producer state. Inputs to one producer SHALL execute serially and reuse that state. Each invocation SHALL have an isolated input context. Outputs SHALL establish a distinct ordered message domain subject to existing dependency and context-reference rules.

#### Scenario: Reuse a Send-only producer

- **WHEN** two inputs execute on one producer containing mutable state that is not Sync
- **THEN** the first invocation completes before the second starts and both access the same instance state

#### Scenario: Reject mixed input and output domains

- **WHEN** a downstream node combines a producer output with its original input message
- **THEN** preparation rejects the cross-domain dependency

### Requirement: Drain producers before completion

A startup producer's return SHALL end its production. Closing a producer's upstream SHALL finish admitted
invocations. In both cases, queued outputs MUST drain before downstream closure and successful workflow
completion. Other live sources SHALL remain independent. Failure or instance disposal SHALL wake blocked
senders, prevent further publication, and wait for started producer calls before releasing their state.

#### Scenario: Close immediately after admission

- **WHEN** an explicit input source closes immediately after admitting a request that produces more results
  than queue capacity
- **THEN** the producer finishes and all outputs drain before the workflow succeeds

#### Scenario: Dispose of a blocked producer

- **WHEN** an unfinished instance is dropped while a producer is blocked in a send
- **THEN** the send fails, the producer can return, and its worker and state are released

#### Scenario: Access admission during cleanup

- **WHEN** producer cleanup checks a closed or failed input handle
- **THEN** the check reports closed admission or failure and workflow cleanup completes

#### Scenario: Finish a source with pending downstream work

- **WHEN** a startup producer returns while its final emission is still queued or executing
- **THEN** its own invocation can finish but workflow success waits for downstream drain and selected output
  delivery

### Requirement: Validate and attribute each emission

Emissions SHALL pass existing output and skip validation before publication. Invalid emissions SHALL fail the instance even if a plugin ignores a send error. Producer errors and panics SHALL identify the node, preserve delivered results, and suppress further publication. Panic failures SHALL retain the synthesized `StreamError` in their source chain. Observations SHALL report invocation identity, successful emission counts, and failure phase.

#### Scenario: Ignore an invalid send

- **WHEN** a producer sends the wrong output type and then returns success after ignoring the error
- **THEN** the instance still fails and the invalid result is never delivered

#### Scenario: Fail after a delivered prefix

- **WHEN** a producer fails after the consumer received earlier results
- **THEN** those results remain effective and the terminal failure identifies the producer

#### Scenario: Inspect a producer panic cause

- **WHEN** a producer panics during execution
- **THEN** the failure identifies that producer, retains the synthesized `StreamError` through the node error source, and includes the panic message in its diagnostic

### Requirement: Preserve generated runner behavior

Compiled streaming runners SHALL support external producer packages with the same emission, capacity, ordering, and shutdown behavior as in-memory execution. Build validation and description MUST NOT execute producer operations.

#### Scenario: Run an external line producer

- **WHEN** a workflow using an external producer compiles and runs with more file lines than queue capacity
- **THEN** it emits every line in order and validation and description do not access the input file

### Requirement: Keep startup producers independent

Producers activated directly or through startup task dependencies SHALL be able to run concurrently on their
existing dedicated workers. A producer waiting on I/O or output capacity MUST NOT prevent independent startup
producers from being invoked. Inputs and context for each invocation SHALL remain isolated and respect
explicit ancestry.

#### Scenario: Start two long-lived sources

- **WHEN** two initial producers are ready and the first remains active indefinitely
- **THEN** the second can start and emit without waiting for the first to return

#### Scenario: Start sources after independent tasks

- **WHEN** two startup tasks each supply a producer and the first producer waits for an effect of the second
  branch
- **THEN** both branches can progress with one ordinary task worker without serializing their producer
  lifetimes
