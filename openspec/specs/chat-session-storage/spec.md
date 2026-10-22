# chat-session-storage Specification

## Purpose

Provide durable, independent chat sessions with ordered messages and recoverable turn state through shared storage handlers and application-owned workflow nodes.

## Requirements

### Requirement: Initialize portable persistent storage

The chat host SHALL initialize SQLite storage and apply bundled pending migrations before accepting operations. Reopening current storage SHALL preserve existing data. Initialization failures MUST retain typed sources and prevent the host from accepting requests.

#### Scenario: Open a new database

- **WHEN** the host opens a new writable database location
- **THEN** storage is initialized without external migration files or migration tools
- **AND** session operations become available after initialization succeeds

#### Scenario: Reopen an existing database

- **WHEN** the host reopens a database with all bundled migrations applied
- **THEN** sessions, messages, turns, and model calls remain available without duplication

#### Scenario: Fail initialization

- **WHEN** opening the database or applying a pending migration fails
- **THEN** initialization returns a contextual error with its source and no requests are accepted

### Requirement: Expose application storage operations as nodes

The `mf-chat` package SHALL expose typed `mf_chat.*` nodes for initialization, session management, turn transitions including optional title persistence, and usage queries. Direct database APIs and node adapters SHALL call the same storage handlers. Host storage operations MUST NOT construct or execute wrapper workflows. Preparing or inspecting these nodes MUST NOT open storage, apply migrations, or execute queries.

#### Scenario: Prepare without a database

- **WHEN** a structurally valid chat workflow is prepared or inspected with a nonexistent database path
- **THEN** its node contracts are available without creating database files or applying migrations

#### Scenario: Return a direct storage error

- **WHEN** a database API fails to find a turn or execute a storage query
- **THEN** it returns the typed storage error directly without runtime error wrapping

#### Scenario: Attribute a storage failure

- **WHEN** a storage node fails during execution
- **THEN** the error identifies the operation and preserves the database error source

### Requirement: Manage independent sessions

The host SHALL support creating, listing, loading, renaming, archiving, and restoring sessions by stable identifiers. Lists and message histories SHALL support bounded pagination. Archived sessions SHALL remain readable and retain their data, but reject new turns until restored. Unknown identifiers MUST fail explicitly.

#### Scenario: Restore archived sessions

- **WHEN** a session containing successful and failed turns is archived and later restored
- **THEN** its messages and call records are preserved and new turns can be submitted again

#### Scenario: Request a bounded page

- **WHEN** a caller lists sessions or loads messages with a page limit
- **THEN** the result contains at most that limit with a stable continuation boundary

#### Scenario: Load an unknown session

- **WHEN** a caller loads a session identifier that does not exist
- **THEN** the operation reports an explicit missing-session error

### Requirement: Preserve message ordering and context isolation

Messages SHALL retain their session and turn identities and a stable order within each session. Model history SHALL contain only messages from successful turns of the requested session, in order. Current prompts SHALL remain separate from history. Failed, cancelled, interrupted, and running turns MUST NOT contribute model history.

#### Scenario: Continue after a failed submission

- **WHEN** a successful turn is followed by a failed turn and a new submission
- **THEN** the new request includes only the successful turn as history and its own prompt once
- **AND** the failed submission remains readable as an unsuccessful turn

#### Scenario: Load another session

- **WHEN** a caller loads history for session B after using session A
- **THEN** no messages from session A appear in session B's history

### Requirement: Begin a durable turn before model execution

Beginning a turn SHALL atomically persist its user message, running state, and model-call identity before provider execution. A session SHALL admit at most one running conversation turn. No database transaction SHALL remain open while waiting for a provider.

#### Scenario: Submit concurrently to one session

- **WHEN** two submissions attempt to begin a conversation turn for the same session
- **THEN** at most one becomes running and the other receives a session-busy result without a provider request

#### Scenario: Use another session during a pending request

- **WHEN** a model request for session A is waiting for its reply
- **THEN** storage operations for session B can complete without waiting for that provider request

### Requirement: Commit replies and call facts atomically

Successful completion SHALL save the assistant message, reported model-call metadata, optional title, and successful turn state in one transaction. The host MUST NOT report a turn as committed until this transaction succeeds. A failed transaction SHALL leave no partial completed reply or usage update. This transaction MUST NOT schedule title work.

#### Scenario: Fail partway through completion

- **WHEN** saving the assistant message or call metadata fails inside the completion transaction
- **THEN** all completion writes are rolled back and the turn is not reported as successful

#### Scenario: Complete a turn without usage

- **WHEN** a model returns valid text without usage counters
- **THEN** the assistant message and successful turn state are committed while missing counters remain unknown

### Requirement: Distinguish persistence retries from model retries

Repeating a completed persistence operation with the same call identity and equivalent result SHALL return the saved result without duplicating messages or usage. A conflicting result for that identity MUST fail. Resubmitting a failed turn SHALL create fresh turn and call identities with a link to the original turn.

#### Scenario: Retry a completion write

- **WHEN** the same completion is submitted twice with the same call identity
- **THEN** only one assistant message and one set of call facts exist

#### Scenario: Reuse an identity with different output

- **WHEN** a completed call identity is submitted with a different reply or usage
- **THEN** the conflicting completion is rejected without altering its saved result

#### Scenario: Request another model attempt

- **WHEN** a failed submission is retried through a new model request
- **THEN** the new turn and call have distinct identities and the original unsuccessful turn is retained

### Requirement: Finalize failed and interrupted work

Failure and cancellation SHALL finalize running turns and calls without adding assistant messages to model history. On restart, the owning host SHALL mark abandoned running work interrupted without resending requests. Late failure or cancellation finalization MUST NOT overwrite an already committed success.

#### Scenario: Reopen after a crash

- **WHEN** the previous owning host exits with running work and storage is reopened
- **THEN** that work becomes interrupted and remains available for explicit retry
- **AND** no provider request is automatically repeated

#### Scenario: Race cancellation with committed completion

- **WHEN** failure or cancellation finalization reaches a turn whose completion is already committed
- **THEN** the saved success and its call facts remain unchanged

### Requirement: Coordinate host ownership before recovery

One database SHALL have at most one active owning chat host. Ownership SHALL be acquired before migrations or abandoned-work recovery and held until active work is joined. An overlapping owner MUST fail explicitly without modifying another host's active turns.

#### Scenario: Open overlapping hosts

- **WHEN** a second owning host opens a database while the first owner is active
- **THEN** the second open fails without migrating storage or marking the first owner's calls interrupted

#### Scenario: Release ownership after shutdown

- **WHEN** the owner finishes or finalizes its active work and closes
- **THEN** another host can acquire ownership and reopen the preserved database

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

### Requirement: Derive successful conversation state from turns

The backend SHALL use persisted successful turns to determine whether a session has completed a conversation. It SHALL NOT expose or maintain a separate conversation_revision counter. Forward migration SHALL remove that derived counter without changing stored turns or messages; development rollback SHALL reconstruct it from successful turns.

#### Scenario: Complete and replay a turn

- **WHEN** a successful completion is saved and then replayed
- **THEN** one successful turn and one assistant message remain, without updating a separate completion counter

#### Scenario: Reject a composed title write

- **WHEN** a database write fails while applying a valid title in a composed completion
- **THEN** the assistant message, usages, title, and successful turn state roll back together
- **AND** the retained final result can be saved again without repeating model work

### Requirement: Distinguish provisional display titles from manual names

Hosts MAY supply a provisional placeholder when creating a session. A placeholder SHALL remain eligible
for workflow-generated replacement; an explicitly manual name SHALL take precedence over generated titles.

#### Scenario: Create a terminal conversation from a prompt

- **WHEN** the terminal uses the first prompt line as its initial display title
- **THEN** the session title source is provisional rather than manual
