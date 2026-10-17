## ADDED Requirements

### Requirement: Persist new replies using usage lists only

Chat replies SHALL expose usages without a parallel scalar usage field. New reply persistence SHALL write only the explicit usage list and SHALL NOT populate legacy token columns. Forward migration SHALL convert successful single-model records with reported legacy counters into one call fact with their stored call ID. Existing lists, sparse counters, explicit zeroes, and historical scalar values SHALL be preserved.

#### Scenario: Save a new model reply

- **WHEN** the embedded host or executable backend saves a reply with call facts
- **THEN** usages contains those facts and legacy scalar token columns remain empty

#### Scenario: Upgrade a scalar-only successful reply

- **WHEN** a legacy successful call has a model and reported token counters but no usage list
- **THEN** one usage record retains its call identity, model, start time, and only the reported counters
- **AND** replay and statistics use the migrated list without double counting the historical columns
