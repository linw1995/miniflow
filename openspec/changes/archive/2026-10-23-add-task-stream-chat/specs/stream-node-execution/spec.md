## MODIFIED Requirements

### Requirement: Block sends under downstream pressure

Each connected send SHALL transfer one validated result into bounded delivery capacity. A full queue SHALL suspend the sender until capacity becomes available or the instance fails. Successful sending SHALL mean admission rather than downstream completion. Blocked producers MUST NOT prevent downstream tasks or timers from running. Unconnected streams SHALL discard validated emissions without allocating a pending queue or requiring an explicit discard consumer.

#### Scenario: Stop consuming with one task worker

- **WHEN** a producer fills its output queue while the consumer pauses and only one ordinary task worker is configured
- **THEN** sends wait within the configured bound and resume in order when consumption continues

#### Scenario: Preserve timer progress

- **WHEN** a producer is executing or waiting for output capacity while a collector deadline becomes due
- **THEN** the runtime can deliver the timer independently

#### Scenario: Produce without an output consumer

- **WHEN** a producer has no graph consumer or selected output delivery for its emissions
- **THEN** valid emissions are discarded without blocking and its invocation and side effects still execute

## ADDED Requirements

### Requirement: Preserve validation when discarding emissions

Discarding output SHALL NOT suppress malformed emission, encoding, cancellation, or ignored publication errors. Consumer absence SHALL be queryable so a provider can emit zero values without constructing them. A connected delivery path MUST NOT become discard after an unexpected receiver closure.

#### Scenario: Reject invalid unconnected output

- **WHEN** a typed producer emits a non-finite field into an unconnected stream
- **THEN** conversion fails with the producer identity and typed source before discard

#### Scenario: Omit all emissions

- **WHEN** a provider observes no consumers and returns without emitting
- **THEN** the invocation completes normally without synthesizing a message
