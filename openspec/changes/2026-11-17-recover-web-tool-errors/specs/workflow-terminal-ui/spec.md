## ADDED Requirements

### Requirement: Keep terminal failure diagnostics distinct

The chat terminal presentation SHALL deduplicate exact diagnostic lines received from multiple terminal error channels while preserving distinct root causes. This presentation rule SHALL NOT alter raw logs or persisted audit facts.

#### Scenario: Avoid repeated wrapper errors

- **WHEN** reconciliation, capture errors, or stderr repeat the same wrapper diagnostic
- **THEN** the chat outcome displays that exact line once
- **AND** a distinct provider or runtime cause remains visible
