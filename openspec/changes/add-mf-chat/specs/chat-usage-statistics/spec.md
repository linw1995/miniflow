# Chat Usage Statistics

## Purpose

Account for model calls from conversation and title workflows and expose reproducible usage summaries that distinguish reported token counts from unknown consumption.

## ADDED Requirements

### Requirement: Preserve call identity and purpose

Each model-call record SHALL retain its stable call identity, session identity, actual model configuration identity, purpose, and lifecycle timestamps. Conversation calls SHALL link to their turn. Title calls SHALL be distinguishable from conversation calls. A scheduled call that has not begun MUST NOT count as an attempted model call.

#### Scenario: Count both application call types

- **WHEN** a session completes one conversation call and one title call
- **THEN** statistics can report two attempted calls overall and one for each purpose

#### Scenario: Query a pending title request

- **WHEN** a title request is scheduled but has not begun execution
- **THEN** it does not increase attempted-call counts or reported token subtotals

### Requirement: Store usage with the corresponding response

Reported usage SHALL be saved with the response persistence transaction for conversation and title calls. Persisted call facts SHALL be the statistics source of truth. Repeating persistence with the same identity MUST NOT add usage twice. Statistics execution MUST NOT be required to finish a conversation.

#### Scenario: Query after a statistics failure

- **WHEN** a response commits and an unrelated statistics query later fails
- **THEN** its reported usage remains saved and appears when the query succeeds again

#### Scenario: Retry response persistence

- **WHEN** equivalent response persistence is repeated for one completed call
- **THEN** aggregates still include that call's reported usage only once

### Requirement: Preserve sparse and unknown token counts

Storage SHALL distinguish absent input, output, and total token counters from explicitly reported zero. Each aggregate SHALL expose a known subtotal, a reported-call count, and a missing-call count for each counter. A subtotal with no reported observations SHALL be unknown. Partial coverage MUST NOT be presented as complete usage.

#### Scenario: Aggregate sparse usage

- **WHEN** one call reports all counters and another reports only output tokens
- **THEN** output usage includes both observations while input and total subtotals identify one missing observation each

#### Scenario: Receive no usage counters

- **WHEN** attempted calls have no reported token counters
- **THEN** their token subtotals are unknown rather than zero and their missing-call counts are visible

#### Scenario: Preserve an explicit zero

- **WHEN** a provider explicitly reports zero for a supported counter
- **THEN** that observation contributes a known zero and increases its reported-call count

### Requirement: Include failed attempts without inventing remote usage

Statistics SHALL include begun calls by terminal status, including failed, cancelled, and interrupted attempts. Calls without reported usage SHALL remain unknown. Local cancellation and process interruption MUST NOT be treated as proof of zero remote consumption or guaranteed remote termination.

#### Scenario: Interrupt a provider attempt

- **WHEN** a begun model call is interrupted before usable usage metadata is available
- **THEN** the attempted-call count and interrupted status are retained while token usage remains unknown

### Requirement: Query consistent scoped aggregates

The statistics flow SHALL support all sessions or one session, an optional model configuration and purpose filter, and an optional UTC time range based on call start time. It SHALL return consistent aggregates from one storage snapshot, with bounded grouping by session, model configuration, purpose, or UTC day. Numeric overflow MUST fail explicitly.

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

### Requirement: Execute statistics as an embedded deterministic flow

The host SHALL obtain statistics through the embedded usage statistics flow and application query nodes. It SHALL aggregate persisted facts without issuing model requests or updating billing facts. Results SHALL remain reproducible for unchanged stored data and identical filters.

#### Scenario: Query without provider credentials

- **WHEN** statistics are requested with model credentials unset
- **THEN** the query succeeds from storage without contacting a provider

#### Scenario: Count a discarded generated title

- **WHEN** a title response is valid but its title update is skipped after a manual rename
- **THEN** its saved reported usage remains included under purpose `title`
