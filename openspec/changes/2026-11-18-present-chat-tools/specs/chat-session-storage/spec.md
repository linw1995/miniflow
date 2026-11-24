## ADDED Requirements

### Requirement: Preserve displayed drafts across interruption

The terminal SHALL commit stream prefixes to turn-scoped display storage before showing them. Drafts MUST NOT become audit messages, model context, usage facts, or executable tool calls.

#### Scenario: Reopen and retry an interrupted turn

- **WHEN** a chat owner exits after displaying text or tool arguments and a later owner retries the turn
- **THEN** the original turn retains its unfinished prefixes in first-seen order
- **AND** the new turn has independent storage and its request contains no draft data

#### Scenario: Replace drafts atomically

- **WHEN** a terminal result is saved
- **THEN** the same transaction deletes the drafts it replaces, retaining unrepresented parts of an unsuccessful turn
- **AND** if the transaction rolls back, neither the result nor draft cleanup takes effect
