## MODIFIED Requirements

### Requirement: Preserve and aggregate observed call performance

Chat SHALL persist optional per-call timing with its response. Queries SHALL expose observed latency
and both output-token rates with explicit observation counts, preserving unknown historical values.

#### Scenario: Query two throughput windows

- **WHEN** a successful call reports output tokens, positive request duration, and first-output latency
- **THEN** end-to-end rate divides output tokens by request duration
- **AND** generation rate divides the same count by request duration minus first-output latency

#### Scenario: Aggregate calls with different durations

- **WHEN** matching calls have different counters and durations
- **THEN** rates use sums of paired counters and positive durations rather than means of individual rates
- **AND** missing counters, failed attempts, and nonpositive windows are excluded with missing counts

#### Scenario: Reopen historical data or retry saving

- **WHEN** a database containing older usage records is upgraded or an identical response is saved again
- **THEN** old timing remains unknown and previously saved timing is neither overwritten nor counted twice

#### Scenario: Reject inconsistent external timing

- **WHEN** a result reports negative timing or first-output latency greater than request duration
- **THEN** persistence fails validation before changing the stored response

### Requirement: Derive request performance from source observations

Chat hosts SHALL correlate request boundary and first-output observations by call identity and derive timing from their source timestamps. Embedded hosts SHALL collect events locally without requiring an external exporter. Executable hosts and graph presentation SHALL use the same aggregation. Hosts SHALL enrich reported call facts before settlement and preserve enriched facts during recovery.

#### Scenario: Aggregate delayed or reordered observations

- **WHEN** matching request observations arrive in a different order or are retransmitted
- **THEN** source timestamps determine timing and retransmission does not duplicate call facts

#### Scenario: Receive incomplete or inconsistent observations

- **WHEN** boundary events are missing, duplicated with conflicting facts, or have noncausal timestamps
- **THEN** affected timing remains unknown and the host does not substitute receipt time

#### Scenario: Reopen an enriched conversation

- **WHEN** a conversation with event-derived timing is saved and reopened
- **THEN** history retains the saved timing and throughput uses the original observations
