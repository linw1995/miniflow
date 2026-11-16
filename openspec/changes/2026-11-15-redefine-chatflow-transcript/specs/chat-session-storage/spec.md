## MODIFIED Requirements

### Requirement: Preserve message ordering and context isolation

Conversation records SHALL retain storage, session, and turn identities with stable session sequence.
Multiple assistant messages and tool results MAY occur in one turn; order MUST NOT assume alternating
roles or two positions per turn. Calls, arguments, results, content, and metadata SHALL remain intact.

Model context SHALL be a separately revisioned view published only by successful settlement. Append
SHALL extend admitted context with the current user and delta. ReplaceContext SHALL publish its full
snapshot without adding input or display data. Context queries MUST NOT reconstruct omitted records
from the conversation transcript. UI queries MUST NOT substitute model context for original messages.
Failed, cancelled, interrupted, and running turns SHALL NOT publish context. The next request SHALL
append its newly admitted user once without reintroducing old system instructions.

#### Scenario: Continue after a successful Agent turn

- **WHEN** a turn completes a tool batch and final answer using append before the next submission
- **THEN** the next request contains that exchange and its new user exactly once

#### Scenario: Continue after a failed submission

- **WHEN** a failed branch-selected result saves assistant/tool audit messages
- **THEN** message pages retain them with the failed state but active model context is unchanged

#### Scenario: Load another session

- **WHEN** session B's model context is loaded after session A has used tools
- **THEN** A's transcript and context cannot enter B's request

#### Scenario: Page inside a tool exchange

- **WHEN** a bounded transcript page starts or ends inside a tool exchange
- **THEN** stable continuation sequences preserve each original record without duplication or reordering

### Requirement: Begin a durable turn before model execution

Admission SHALL atomically record the original user, running turn, application call identity, and
immutable base context revision before model/tool execution. Contract-backed admission SHALL also
retain its advertised result alternatives for recovery without the original manifest. It SHALL allocate only the user's
audit position, not reserve one future assistant or publish running input into model context.
A session SHALL admit at most one running turn. No transaction SHALL remain open while waiting
for a model or tool.

#### Scenario: Submit concurrently to one session

- **WHEN** two submissions attempt to begin one session's turn
- **THEN** at most one is admitted and the other is busy without model/tool execution

#### Scenario: Use another session during a pending request

- **WHEN** session A is waiting for a model or tool
- **THEN** storage operations in session B can complete without waiting for that operation

### Requirement: Commit replies and call facts atomically

Settlement SHALL match the update variant to select its display delta and candidate context. One
transaction SHALL append only that delta, save the exact selected result and facts, advance audit allocation,
and finalize turn/call state. Success SHALL publish the variant-derived context against the admitted
revision and MAY apply an eligible title. Failure SHALL retain audit/candidate data but publish no
context or title; a newer active context alone SHALL NOT block failed audit persistence. Invalid
zero/multiple-alternative results SHALL NOT enter settlement or become save-only candidates.

A transaction failure or stale successful base MUST leave no partial messages, facts, title,
allocation, terminal state, or context publication. Allocation SHALL use display-message count,
not replacement snapshot length. The host MUST NOT report settlement before commit.

#### Scenario: Fail partway through completion

- **WHEN** storage fails after inserting an assistant batch but before results, accounting, or context publication
- **THEN** all settlement writes roll back and the original update remains eligible for save-only retry

#### Scenario: Complete a turn without usage

- **WHEN** a successful update reports an attempt without token counters
- **THEN** display messages and context commit while unreported counters remain unknown

#### Scenario: Settle a failed result with no generated messages

- **WHEN** the display delta is empty in a failed result with explicit usages
- **THEN** failure state and facts commit without a placeholder assistant or context revision

#### Scenario: Detect sequence overflow

- **WHEN** audit sequence or context revision advancement would overflow
- **THEN** settlement fails without partial messages or publication

#### Scenario: Keep snapshot size out of display allocation

- **WHEN** a replacement contains many context messages but one display message
- **THEN** only one new display position is allocated and context-only records do not enter message pages

### Requirement: Persist opaque message metadata and execution call facts

Conversation records SHALL have canonical role-tagged message payloads and optional object metadata.
ReplaceContext SHALL NOT overwrite prior display/audit records. Exact result storage SHALL preserve
the selected alternative, every payload field, usages, title, and full errors even if display summaries
are bounded. Accounting SHALL count model attempts rather than messages or context copies.

Forward migration SHALL preserve sessions, original message IDs and sequence gaps, annotations,
titles, legacy facts, and ownership. Text records SHALL become equivalent canonical payloads.
Initialize model context once from nonempty system instructions and successful legacy exchanges;
backfill known results as append updates when possible. Do not invent old tool exchanges, unknown
attempts, or complete results for abandoned work.

#### Scenario: Reopen annotated history

- **WHEN** a saved Agent turn using append is reopened
- **THEN** original messages and model context retain calls, results, order, and metadata
- **AND** usage totals are independent of message count

#### Scenario: Upgrade an existing database

- **WHEN** storage predates alternative result outputs and separate model context
- **THEN** migration preserves readable original conversations and known/unknown accounting without executing a Flow

#### Scenario: Ignore rewritten returned history

- **WHEN** a replacement snapshot edits earlier messages or summarizes their tool output
- **THEN** original display/audit records remain unchanged while only model context is replaced
- **AND** no new tool effects or changed charges are inferred from snapshot content

### Requirement: Distinguish persistence retries from model retries

Equivalent canonical results SHALL replay without duplicate display messages, facts, or context
publication. Different update kinds or payloads MUST conflict even if the final answer or derived
context is equal. Saved replay SHALL be checked before active-revision comparison. A fresh model
retry SHALL have new linked invocation identities, use current published context, and neither resume
failed transcript data nor claim previous external tool effects were rolled back.

#### Scenario: Retry a completion write

- **WHEN** the same branch-selected result is settled twice for one invocation
- **THEN** one copy of its display delta and facts exists with at most one published context revision

#### Scenario: Reuse an identity with different output

- **WHEN** replay changes kind, display/context messages, tool data, metadata, title, or usages
- **THEN** settlement rejects it without altering saved state

#### Scenario: Request another model attempt

- **WHEN** an unsuccessful submission is retried as new generation
- **THEN** fresh identities retain the retry link and the original audit result remains inspectable

### Requirement: Derive successful conversation state from turns

Successful turns SHALL determine whether a session completed a conversation without a separate
conversation_revision counter. Context revision SHALL be an independent state token for conditional
publication, not a duplicate success count. Display persistence, terminal state, facts, title, and
context publication SHALL remain one atomic successful settlement.

#### Scenario: Complete and replay a turn

- **WHEN** a successful normalized result is saved and replayed
- **THEN** one successful turn and one copy of each display message remain without a separate completion counter

#### Scenario: Reject a composed title write

- **WHEN** applying a title fails during settlement
- **THEN** audit, facts, title, state, allocation, and variant-derived context roll back together
- **AND** retry retains the original result without backend execution

## ADDED Requirements

### Requirement: Maintain model context independently of conversation records

Sessions SHALL retain an active model-context revision separately from the append-only displayed
transcript. Admission SHALL retain its immutable base for recovery. Append revisions SHALL reference
the user and display delta; replace revisions SHALL store exactly the complete snapshot. Context
materialization SHALL use checkpoints and append references, not replay all audit data or duplicate
full context each turn. Initial system instructions SHALL enter revision zero once.

#### Scenario: Reopen a compacted context

- **WHEN** replacement compacts old and current tool output and storage is reopened
- **THEN** model_context returns exactly that snapshot while transcript queries retain all original display messages
- **AND** subsequent append revisions extend the chosen snapshot rather than restoring discarded messages

#### Scenario: Clear model context but retain this turn on screen

- **WHEN** successful replace_context has an empty snapshot and a valid display answer
- **THEN** active model context is empty, including no automatic current user or display suffix
- **AND** the transcript retains the admitted user and display answer

#### Scenario: Preserve rewritten system instructions

- **WHEN** a replacement changes or omits the initial system message
- **THEN** later requests do not prepend legacy instructions again

#### Scenario: Recover against the admitted base

- **WHEN** a retained append result is decoded after context changes
- **THEN** validation can load its original request revision and user without an executable or audit-history reconstruction

### Requirement: Publish context with revision-checked settlement

Success SHALL compare active context revision with the stored admitted base and atomically publish
one new revision with audit/result/facts/title. A mismatch MUST reject all settlement writes without
automatic merging or rebasing. Equivalent settled replay SHALL return before this comparison and
MUST NOT move context backward. Failed results, cancellation, and abandon SHALL NOT publish a
candidate. Title-only changes SHALL not invalidate context.

#### Scenario: Reject a stale successful replacement

- **WHEN** active context no longer matches a successful result's admitted base
- **THEN** a typed conflict leaves all settlement writes and the newer context unchanged
- **AND** storage retry does not silently rebase or rerun the Flow

#### Scenario: Fail or cancel after constructing a replacement

- **WHEN** a candidate replacement is computed but failure/cancellation prevents successful settlement
- **THEN** no candidate context is published even if the valid branch-selected result is retained for audit

#### Scenario: Persist failed audit after a context change

- **WHEN** a failed result is saved after another operation changes active context
- **THEN** original audit and usages can settle without publishing or overwriting context

#### Scenario: Replay an old successful settlement

- **WHEN** a saved result is replayed after newer turns
- **THEN** display/facts are not duplicated and active context does not move backward

#### Scenario: Rename during replacement generation

- **WHEN** only the session title changes while a Flow is generating a replacement
- **THEN** the context base remains valid and title revision checks preserve the manual name
