# Design

## Implementation status

The implementation replaces the earlier telemetry-only, render/checkpoint-polling, whole-turn JSON draft. Optional
canonical messages, typed append events, the shared reducer, related tables and event-driven ToolUse consumers are
implemented together. Verification of this revision is recorded separately in `verification.md`.

`ConversationResult.messages` reuses `mfn_llm::ChatMessage`. The standard Agent exposes only its current invocation's
new messages. `llm.message_events` forwards checked `Shared<MessageBatch>` values so publishing does not erase native
sensitivity. Assistant and result publishers retain separate native domains (`message_events` and
`tool_message_events`), with invocation-scoped offsets and terminal-list reconciliation.

## Two sources with different authority

1. **Flow messages define conversation semantics.** Assistant tool-call messages establish requests, names, submitted
   arguments, and message order; tool-result messages establish returned content and its link to a request.
2. **Execution observations define runtime facts.** Existing validated tool telemetry supplies dispatched/running/
   failed/interrupted states, execution identity, resolved arguments, and measured durations. Such observations must
   also remain visible if the Flow emits no corresponding messages.

Neither source replaces the other. A tool-result message means a result was returned, not necessarily that execution
succeeded. An error-looking string is not a typed failure. Parent/model success does not prove tool success.
Keep conversation state (`requested`, `returned`, `unresolved`) separate from execution state. A returned result with
missing telemetry is displayed as returned with execution unobserved; failure telemetry plus a returned result displays
both facts. Submitted arguments and resolved execution arguments are distinct fields when they differ.

Message values and observations pass their existing privacy/redaction boundaries independently. Joining sources must
not recover a protected value from another source. Tool strings remain bounded, control-safe literal text, not
Markdown commands or image requests.

## Structured message contract

Add optional terminal `messages` and typed `message_events`/`tool_message_events` streams without making them mandatory for
existing chat Flows. Reuse `ChatMessage` roles/tool-call values rather than parsing text into calls.

- `messages` contains this invocation's new messages. Existing system/history input and the already-admitted user
  prompt are not returned as new messages. The builtin Agent adapts its internal full-context list at this boundary;
  the host must not guess a prefix from equal message text.
- Message events have stable per-invocation sequence/message keys and carry structured message values. An explicit
  originating model-call ID is carried where available. Producer fragments are not committed as complete arguments.
- Append/replay is idempotent. A final message list reconciles with the same sequence, rather than adding a second set
  of calls. Conflicting revisions and invalid ordering are explicit errors/uncertainty, not last-writer-wins updates.
- Terminal-only Flows may omit the stream. Older Flows may omit both outputs and still expose observation-only tools.

## Durable messages are the session recovery source

Every Flow-produced message accepted by the `mf-chat` contract must be persisted, not merely inspected to extract
ToolUse. The ordered canonical messages are the recovery source; ToolUse is a linked projection and telemetry is
additional evidence. Losing a valid assistant/tool message while retaining its display card is not successful storage.

Persist all accepted roles, complete content, message metadata, assistant tool calls/arguments, and tool-result
identities in order. The existing `messages` table must support multiple assistant/tool messages in a turn and
assistant messages with tool calls but empty text. Remove its one-message-per-role assumption and stop reserving
exactly two message positions per turn. Allocate stable session order transactionally for each accepted new message,
with a unique source-message key scoped to its owning invocation. Replays reuse that mapping and never append a
second message. Existing message IDs/order remain valid through migration.

Canonical recovery data is not the lossy tool display summary. A 2048-byte display limit or 64-item observation window
must not truncate or discard otherwise valid message content/calls. Preserve the accepted message structure according
to existing chat-storage/privacy rules; reject data that cannot be persisted safely rather than silently substituting
a redacted/truncated display copy and reporting successful recovery. Public diagnostic copies remain bounded/redacted.

On restart, history loading reconstructs the full successful-turn `ChatMessage` sequence, including assistant
`tool_calls`, tool results, and metadata, rather than only user/final-text pairs or reconstructed ToolUse summaries.
Keep the existing session isolation and unsuccessful-turn exclusion for model context: failed/cancelled/interrupted
message prefixes remain available in the UI/recovery record, but are not silently fed into the next model request.
Metrics-only records never become model-history messages.

Message events may be persisted during execution in short transactions. Persist each message, its semantic call/result
relations, and the applied source cursor atomically. Final settlement checks the terminal message result and commits
message completeness, final result/usage/title, and successful turn state together. A storage failure retains the full
pending message changes with save-only recovery; no caller may report the turn saved because only `text` was stored.
Canonical append events use reliable bounded backpressure, not latest-value coalescing that could drop a message.
Execution-status updates may still coalesce by identity without blocking on frontend rendering.

If `messages` is supplied, its final assistant record must represent the canonical final reply; a postprocessing Flow
must make the exposed final message consistent with `text`. The host must neither silently rewrite the transcript
from raw model text nor append another assistant message from `text`. Only a legacy Flow omitting `messages` retains
the existing admitted-user plus final-text fallback. Historical input and the admitted prompt are never inserted twice.

## Identity, correlation, and merged projection

Use a host-owned stable tool identity scoped by conversation invocation, originating assistant/model-message identity,
and provider tool-call ID. Provider IDs alone are not globally unique. Reuse of an ID in another model round, retry,
turn, or session must not collide.

An execution observation has its own identity, scoped by conversation invocation, actual run, and runtime invocation/
item path. Preserve that identity even when it links to a semantic call. One semantic call may have multiple execution
attempts; durations and failures remain attached to their attempts, not summed or overwritten arbitrarily.

Link sources only through explicit origin/correlation metadata. The standard Agent should propagate model-call/tool
origin to dispatch observations outside the provider's argument/schema payload. A separate origin producer depends
only on usage identity, not the growing business-message history; conservative CEL sensitivity must not erase public
provenance in later rounds. Legacy telemetry without enough
identity stays unlinked; tool name, parameter similarity, timestamps, or arrival order are not association rules.

| Available evidence | Presentation |
| --- | --- |
| Messages only | One semantic tool item, request/result content, execution facts explicitly unknown |
| Observations only | A labeled execution-observation item; do not pretend a model requested it |
| Both with a proven link | One semantic item enriched with its execution attempt(s), not two tool calls |
| Both without a proven link | Preserve both, explicitly unassociated; do not guess or claim a combined count |
| Contradictory source facts | Retain source-specific content/state and show the discrepancy |

Messages determine semantic tool order within the turn. Unlinked observations remain in a clearly labeled execution
section associated with that turn, rather than being inserted as fabricated assistant/tool messages. Late association
must unify the visible item without losing source evidence, identity-dependent UI state, or the reader's position.
If late facts remove uniqueness, clear the association without deleting either source's rows or guessing a replacement.
Counts distinguish semantic calls, execution attempts, and unlinked observations.

## Event-driven collection and display

The path is producer event -> shared reducer -> changed-tool event -> persistence subscriber and terminal subscriber.
No ToolUse render callback reads observation snapshots. No timer, cancellation callback, or repeated revision check drives tool
collection. Existing UI animation and process-supervision timers are not ToolUse data sources.

The observation receiver publishes accepted lifecycle/value changes and closure. Subscription bootstrap and live
handoff must not miss a fast child. Message events enter the same reducer through the executable decoder. The reducer
keeps bounded source state, handles duplicate/out-of-order events, and emits only actual changes.

Persistence and rendering are independent consumers. Coalesce pending updates by tool/execution identity, not by
silently dropping terminal states. A paused frontend or hidden panel cannot block receiver draining or cancellation.
Final settlement drains pending events and persistence before releasing ownership. A restarted application restores
rows and their source links; it does not subscribe to an old process or replay a workflow.

## Relational persistence and write amplification

Remove the live `model_calls.tool_uses` column. Use related tables, conceptually:

- `messages`: authoritative ordered message rows, with role/content/metadata and stable invocation/source-message
  identity. Evolve this existing table rather than persisting a second whole-transcript JSON copy.
- `chat_tool_calls`: stable semantic ID, conversation-call FK, originating persisted assistant-message FK and model
  identity, provider tool-call ID, ordinal, name, and complete submitted argument structure. A tool-result association
  links to the persisted tool-message row; returned content is not duplicated in a display-summary blob. A small
  immutable redacted argument preview avoids exposing canonical protected values in display queries. Sensitivity
  paths for arguments/message metadata survive reopen alongside the complete canonical data.
- `tool_executions`: execution ID and run/scope identity, owning conversation-call FK, nullable semantic-call FK,
  source correlation, resolved arguments, observed result/state, and duration. Multiple attempts are separate rows.
- A small per-invocation/source checkpoint records sequence/completeness/end state. Its presence distinguishes an
  observed empty source from legacy unrecorded data without a whole-turn JSON document.

Unique constraints enforce source-scoped identity and idempotent replay; foreign keys enforce ownership. Late links
update the association, not copies of the content. Bounded message-page queries batch-load related calls/executions;
model-call reads and usage updates do not load or rewrite tool payloads. History reconstruction joins canonical
message/call/result relations, while usage is attached once to the turn's final response rather than repeated on every
intermediate assistant message.

Insert a new call once. Update only changed columns in the affected call/execution row. A phase/duration update must
not rewrite arguments/results or touch sibling calls, and exact replay must perform no payload write. Batch adjacent
deltas transactionally while keeping cancellation-aware, fail-fast storage behavior and save-only recovery.

Migration must preserve the current draft's already-recorded data. Its JSON journals originated from telemetry, so
migrate them as legacy observation-only records with unknown missing origin/run fields left unknown. Do not invent
Flow messages or associate them by name. Preserve the existing IDs/public facts before removing the obsolete column;
invalid data must fail migration rather than disappear. The final schema has no tool-journal column on model calls.

## Validation plan

Cover messages-only, observations-only, linked sources in both arrival orders, late association, repeated/reused IDs,
multiple attempts, cancellation before a tool result, contradictory facts, and absent source correlation. Verify exact
replay/final-list reconciliation does not duplicate messages or calls, historical context is not reingested, final
canonical text is not replaced, and protected values stay protected. Round-trip multiple assistant/tool messages,
empty-text tool-call assistants, metadata, large valid payloads, and valid call counts above the display/observation
window. After restart, submit another turn and assert the provider receives the complete prior successful message
sequence, not just a visually restored tool card or user/final-text pair.

Use database triggers/update instrumentation to prove a status-only change writes no payload columns or sibling rows
and that replay writes nothing. Test migration of legacy JSON as observation-only records, rollback/reopen, scoped FKs,
nonblocking contention, and save-only recovery. Verify event delivery without rendering or time-based polling, fast
subscription startup, bounded backpressure, cancellation, final draining, and real terminal restart.

## Scope boundary

Only ToolUse is in this PR. No desktop frontend, graphical dependencies, theme, package-license exceptions, or
packaging changes are introduced. The new design requires implementation and fresh validation before readiness.
