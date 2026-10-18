## RENAMED Requirements

- FROM: `### Requirement: Execute statistics as an embedded deterministic flow`
- TO: `### Requirement: Query deterministic statistics directly`

## MODIFIED Requirements

### Requirement: Query consistent scoped aggregates

The statistics query SHALL support all sessions or one session, optional profile, model, and purpose filters, and an optional UTC time range based on call start time. It SHALL return consistent aggregates from one storage snapshot, with bounded grouping by session, profile, model, purpose, or UTC day. Numeric overflow MUST fail explicitly.

#### Scenario: Scope to one session and purpose

- **WHEN** statistics are requested for session A with purpose `conversation`
- **THEN** session B and title calls are excluded from the result

#### Scenario: Query a UTC interval

- **WHEN** a query supplies a start and end time
- **THEN** calls starting at the inclusive start are included and calls starting at the exclusive end are excluded

#### Scenario: Query an empty interval

- **WHEN** no begun calls match the requested filters
- **THEN** the overall call count is zero, token subtotals are unknown, and grouped results are empty

#### Scenario: Exceed the aggregate range

- **WHEN** summing reported counters would exceed the supported integer range
- **THEN** the query returns an explicit overflow error instead of wrapping or saturating

### Requirement: Query deterministic statistics directly

The host SHALL query statistics directly through the shared storage handler. The application query node SHALL adapt that same handler for workflows without creating a nested flow. It SHALL aggregate persisted facts without issuing model requests or updating billing facts. Results SHALL remain reproducible for unchanged stored data and identical filters.

#### Scenario: Query without provider credentials

- **WHEN** statistics are requested with model credentials unset
- **THEN** the query succeeds from storage without contacting a provider

#### Scenario: Count a discarded generated title

- **WHEN** a title response is valid but its title update is skipped after a manual rename
- **THEN** its saved reported usage remains included under purpose `title`
