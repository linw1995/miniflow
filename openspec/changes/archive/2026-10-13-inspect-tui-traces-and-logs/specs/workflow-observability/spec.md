## ADDED Requirements

### Requirement: Correlate opt-in snapshots with active spans

Opt-in snapshot LogRecords emitted under a valid active span SHALL preserve its native trace and span IDs. All fragments of one snapshot record SHALL preserve the same association. Existing snapshot bodies, sequence, digest, value references, and workflow/run identities SHALL retain their meanings. Correlation MUST NOT require embedding full business values in span attributes.

#### Scenario: Export node input and output snapshots

- **WHEN** an observed finite node emits opt-in snapshots while its span context is attached
- **THEN** exported snapshot LogRecords identify that node invocation's trace and span

#### Scenario: Fragment a large node record

- **WHEN** a snapshot record exceeds the single-packet limit
- **THEN** every exported fragment retains the record's active trace/span association

#### Scenario: Export without an active span

- **WHEN** a snapshot is emitted without valid trace context
- **THEN** it retains the normal snapshot contract without inventing a trace/span identity
