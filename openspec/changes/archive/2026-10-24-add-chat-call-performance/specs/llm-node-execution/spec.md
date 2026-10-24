## ADDED Requirements

### Requirement: Report observed model request timing

Each attempted provider operation SHALL report monotonic request duration and optional first-text latency
in its usage fact. Unary calls and streams without nonempty text SHALL preserve unknown first-text latency.

#### Scenario: Receive a streaming answer

- **WHEN** a provider sends empty or metadata-only events before nonempty text
- **THEN** first-text latency is observed at the first nonempty decoded text event before publishing it
- **AND** request duration ends at provider completion before terminal conversion or persistence

#### Scenario: Complete without a stream subscription

- **WHEN** the node executes its unary request path
- **THEN** request duration is recorded while first-text latency remains unknown

#### Scenario: Capture a failed provider attempt

- **WHEN** a provider operation fails and the node returns a captured failure
- **THEN** its observed timing is retained without inventing output-token rates
