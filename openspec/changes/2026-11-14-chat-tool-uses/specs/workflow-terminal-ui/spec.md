# Terminal Chat Tool Uses

## ADDED Requirements

### Requirement: Present tool-use evidence inline in terminal chat

Terminal chat SHALL display actual per-turn tool status, bounded public parameters/results, and observed duration inline in the conversation. Tool presentation SHALL remain available when the execution panel is hidden. Rendering SHALL NOT write storage or infer calls from model text. Unsupported states, absent duration, and incomplete observation SHALL remain explicit.

#### Scenario: Read tools while waiting for a reply

- **WHEN** tool execution has been observed but the model has not finished its reply
- **THEN** the transcript shows that tool's observed status and available public details without waiting for conversation completion

#### Scenario: Hide the execution panel

- **WHEN** the user toggles the execution panel off
- **THEN** inline tool-use evidence remains visible and collection continues

#### Scenario: Render untrusted tool text

- **WHEN** tool details contain terminal controls, Markdown syntax, or image links
- **THEN** details are rendered literally with control characters escaped and do not execute terminal commands or load images

### Requirement: Restore tools without replaying execution

Terminal chat SHALL render stored evidence once per turn using stable invocation/tool identities. Starting another turn SHALL preserve earlier records. Failed or cancelled user-only history SHALL retain its tools without inventing an assistant reply. Missing legacy evidence SHALL be labeled unrecorded, not empty success. Retained evidence SHALL remain visibly unsaved until persistence succeeds.

#### Scenario: Reopen a saved conversation

- **WHEN** terminal chat loads messages after the original reader was released
- **THEN** tool details and phases restore from the shared message page without any model or workflow request

#### Scenario: Page across a turn boundary

- **WHEN** a page contains both user and assistant records or only the assistant record for a turn
- **THEN** the turn's tool journal is shown once in that page and remains associated with the correct turn

#### Scenario: Read cancelled history

- **WHEN** a cancelled turn has a user message and recorded tools but no assistant message
- **THEN** the terminal shows its tools and cancellation state without a fabricated reply

#### Scenario: Receive a stale completion

- **WHEN** an older invocation's completion arrives while a newer turn is active
- **THEN** it cannot replace the newer turn's tool evidence
