# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
