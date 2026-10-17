## MODIFIED Requirements

### Requirement: Commit replies and call facts atomically

Successful completion SHALL save the assistant message, reported model-call metadata, and successful turn state in one transaction. The host MUST NOT report a turn as committed until this transaction succeeds. A failed transaction SHALL leave no partial completed reply or usage update. This transaction MUST NOT schedule title work.

#### Scenario: Fail partway through completion

- **WHEN** saving the assistant message or call metadata fails inside the completion transaction
- **THEN** all completion writes are rolled back and the turn is not reported as successful

#### Scenario: Complete a turn without usage

- **WHEN** a model returns valid text without usage counters
- **THEN** the assistant message and successful turn state are committed while missing counters remain unknown

## ADDED Requirements

### Requirement: Derive successful conversation state from turns

The backend SHALL use persisted successful turns to determine whether a session has completed a conversation. It SHALL NOT expose or maintain a separate conversation_revision counter. Forward migration SHALL remove that derived counter without changing stored turns or messages; development rollback SHALL reconstruct it from successful turns.

#### Scenario: Complete and replay a turn

- **WHEN** a successful completion is saved and then replayed
- **THEN** one successful turn and one assistant message remain, without updating a separate completion counter

#### Scenario: Reject title scheduling after a reply commits

- **WHEN** title request insertion fails after a successful conversation
- **THEN** the reply, usages, and successful turn remain committed and title scheduling can be attempted separately
