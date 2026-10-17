## MODIFIED Requirements

### Requirement: Include failed attempts without inventing remote usage

Statistics SHALL include reported model attempts by terminal status and preserve legacy single-model call facts. Executions without a known single-model identity or returned usage list SHALL be exposed separately as unreported_invocations, not fabricated model-call counts. Calls without reported counters SHALL remain unknown. Local cancellation and process interruption MUST NOT be treated as proof of zero remote consumption or guaranteed remote termination.

#### Scenario: Interrupt a provider attempt

- **WHEN** a known single-model call is interrupted before usable usage metadata is available
- **THEN** the attempted-call count and interrupted status are retained while token usage remains unknown

## ADDED Requirements

### Requirement: Aggregate explicit usage lists without double counting

Statistics SHALL aggregate each reported call by its model, counter presence, and reported start time,
falling back to execution admission for older records. Empty lists mean no reported calls; legacy scalar
facts apply only when a list is absent. Message history and metadata MUST NOT add usage, and retries
MUST NOT duplicate call facts.

#### Scenario: Repeat one node within a turn

- **WHEN** one node produces two distinct reported calls in one workflow execution
- **THEN** aggregates count both attempts and their independently reported counters

#### Scenario: Distinguish unreported execution from an empty list

- **WHEN** one execution returns an empty list and another ends without a final result
- **THEN** only the latter increases unreported_invocations and neither invents model counters

#### Scenario: Attribute a loop across UTC dates

- **WHEN** model calls in one workflow execution start on different UTC days
- **THEN** time filtering and grouping use their individual reported start timestamps
