## ADDED Requirements

### Requirement: Persist opaque message metadata and execution call facts

Messages SHALL support optional object-valued metadata without rewriting prior messages. Successful
completion SHALL persist assistant metadata and the returned usage list in the same transaction as text
and terminal state. Failed workflow results SHALL persist usage lists and failure state without adding
assistant messages. History size accounting SHALL include metadata. Existing databases SHALL migrate
forward without losing messages, legacy usage facts, or ownership constraints.

#### Scenario: Reopen annotated history

- **WHEN** a saved assistant reply has metadata and storage is reopened
- **THEN** subsequent history retains the same annotations

#### Scenario: Ignore rewritten returned history

- **WHEN** a workflow returns changed metadata for an earlier history message
- **THEN** saved history remains unchanged and only the new reply's metadata is persisted

#### Scenario: Upgrade an existing database

- **WHEN** storage predates metadata and usage-list columns
- **THEN** migration preserves legacy call counters and old messages have absent metadata
