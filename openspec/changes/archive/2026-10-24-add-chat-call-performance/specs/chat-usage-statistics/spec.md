## ADDED Requirements

### Requirement: Preserve and aggregate observed call performance

Chat SHALL persist optional per-call timing with its response. Queries SHALL expose observed latency
and both output-token rates with explicit observation counts, preserving unknown historical values.

#### Scenario: Query two throughput windows

- **WHEN** a successful call reports output tokens, positive request duration, and first-text latency
- **THEN** end-to-end rate divides output tokens by request duration
- **AND** generation rate divides the same count by request duration minus first-text latency

#### Scenario: Aggregate calls with different durations

- **WHEN** matching calls have different counters and durations
- **THEN** rates use sums of paired counters and positive durations rather than means of individual rates
- **AND** missing counters, failed attempts, and nonpositive windows are excluded with missing counts

#### Scenario: Reopen historical data or retry saving

- **WHEN** a database containing older usage records is upgraded or an identical response is saved again
- **THEN** old timing remains unknown and previously saved timing is neither overwritten nor counted twice

#### Scenario: Reject inconsistent external timing

- **WHEN** a result reports negative timing or first-text latency greater than request duration
- **THEN** persistence fails validation before changing the stored response
