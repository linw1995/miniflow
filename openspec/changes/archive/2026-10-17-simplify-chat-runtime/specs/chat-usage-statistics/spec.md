## MODIFIED Requirements

### Requirement: Preserve call identity and purpose

Each begun model-call record SHALL retain its stable call identity, session identity, profile and actual model identity, purpose, and lifecycle timestamps. Conversation calls SHALL link to their turn. Title calls SHALL be distinguishable from conversation calls. A scheduled call that has not begun MUST NOT count as an attempted model call.

#### Scenario: Count both application call types

- **WHEN** a session completes one conversation call and one title call
- **THEN** statistics can report two attempted calls overall and one for each purpose

#### Scenario: Query a pending title request

- **WHEN** a title request is scheduled but has not begun execution
- **THEN** it does not increase attempted-call counts or reported token subtotals

### Requirement: Query consistent scoped aggregates

The statistics flow SHALL support all sessions or one session, optional profile, model, and purpose filters, and an optional UTC time range based on call start time. It SHALL return consistent aggregates from one storage snapshot, with bounded grouping by session, profile, model, purpose, or UTC day. Numeric overflow MUST fail explicitly.

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

## ADDED Requirements

### Requirement: Retire configuration fingerprints

Call APIs and usage queries SHALL NOT require or expose configuration fingerprints. Upgrading an
existing database SHALL preserve historical fingerprints as inert storage data, call identities,
lifecycle state, message metadata, reported usage lists, and legacy token counters. New calls SHALL
omit fingerprints, and statistics SHALL NOT group or filter by them.

#### Scenario: Upgrade a database with configuration fingerprints

- **WHEN** an existing database is opened by the updated backend
- **THEN** its calls and usage totals remain available without configuration hashing or grouping
