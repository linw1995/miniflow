# Source-aware Tool Storage

## ADDED Requirements

### Requirement: Persist tool sources in associated rows

Storage SHALL use related semantic-call, execution, and small source-checkpoint records rather than a whole-turn model-call JSON column. Scoped unique keys and foreign keys SHALL preserve identity and ownership. Diagnostic payloads SHALL be bounded/redacted before writing. Absent legacy data SHALL remain unrecorded, not empty success. Diagnostic execution records SHALL NOT enter model-input history implicitly.

#### Scenario: Reopen mixed source data

- **WHEN** a turn contains semantic calls, linked attempts, and unlinked observations
- **THEN** history restores the same source facts and associations without executing the workflow

#### Scenario: Retain a returned result and failed execution

- **WHEN** a semantic tool result was returned while the linked attempt failed
- **THEN** both the returned content and failed runtime state survive restart without one overwriting the other

#### Scenario: Migrate a previous draft journal

- **WHEN** a database contains the earlier telemetry-derived JSON journal
- **THEN** its IDs and public facts migrate as observation-only evidence before the obsolete column is removed
- **AND** no semantic messages, unknown run identity, or guessed source links are fabricated

### Requirement: Persist ordered canonical workflow messages

Valid Flow messages SHALL be stored with stable source identity and session order, not only as ToolUse projections. Storage SHALL support multiple assistant/tool messages per turn, empty-text tool-call assistants, metadata, and complete call/result relations. Successful history loading SHALL reconstruct these messages, while unsuccessful prefixes remain recoverable outside model context. Display limits SHALL NOT truncate canonical recovery data.

#### Scenario: Store more than a user and final assistant pair

- **WHEN** a turn emits several assistant tool calls, tool results, and a final reply
- **THEN** all accepted messages and their relations survive restart in the same order without violating a fixed two-slot or one-message-per-role assumption

#### Scenario: Replay a message during final reconciliation

- **WHEN** an already-persisted streamed message appears again in the terminal message list
- **THEN** its source key resolves to the original message row without a duplicate sequence allocation

#### Scenario: Fail final persistence

- **WHEN** final message reconciliation or turn/usage/title persistence fails
- **THEN** no successful turn is reported and the complete pending message changes remain available for save-only recovery

#### Scenario: Keep diagnostic tool observations outside context

- **WHEN** a turn includes telemetry-only tool executions without corresponding canonical messages
- **THEN** they remain inspectable without fabricated assistant/tool history records

### Requirement: Write tool evidence incrementally

Storage SHALL insert new records and update only changed fields in affected rows. A status/duration change SHALL NOT rewrite stored argument/result payloads or sibling records. Exact replay SHALL be idempotent without payload writes. Source sequences/completion SHALL be tracked independently; message completion SHALL NOT suppress still-arriving execution evidence.

#### Scenario: Update one execution status

- **WHEN** one of several executions advances from running to a terminal state
- **THEN** only its dirty state/duration fields and necessary small checkpoint fields are written
- **AND** instrumentation observes no argument/result-column update or sibling-row update

#### Scenario: Replay unchanged evidence

- **WHEN** already-applied source events or final messages arrive unchanged
- **THEN** no duplicate call/attempt and no redundant payload write occurs

#### Scenario: Link an observation after message completion

- **WHEN** a proven execution correlation arrives after the semantic message source has ended
- **THEN** storage links the existing records without rewriting message content or treating the execution as a new call

### Requirement: Keep recovery independent of execution

Write contention SHALL fail promptly without blocking cancellation. Unpersisted source evidence SHALL remain available for save-only recovery. Interrupted sources SHALL retain known successes and settle only unresolved facts as unknown/interrupted. Completed source facts SHALL not be silently rewritten. Loading, saving, and migration SHALL NOT rerun models or tools.

#### Scenario: Retry a failed write

- **WHEN** storage recovers after retaining a reply and its source records
- **THEN** Save persists the original messages, relations, and execution changes without another model or workflow request

#### Scenario: Reject another turn's update

- **WHEN** a source event names an incompatible session/turn/call identity
- **THEN** the update fails with typed context and changes no associated rows
