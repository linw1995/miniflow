# Design

## Provider and conversation streams

Rig buffers Chat Completions arguments before yielding its tool-call event. The existing SSE normalizer captures nonempty string fragments from the primary choice before that assembly. The synchronous publisher drains them on the same poll that reads the frame, including when Rig is still waiting for the rest of the call. This adds no second loader or provider request. Rig still owns validation and completed model output.

`tool_call_delta` carries raw arguments, model-call identity, provider tool index, and optional wire
ID/name hints. An identity-only record retains its hints with an empty argument string.

The shared Agent may also forward a tool Flow's optional string `progress` as `tool_output`. The
caller supplies the model call and tool call identities; they are not parsed from command output.
Each update replaces a bounded display snapshot. It is not a model delta, execution status, or tool
result. `llm.tool_result` still consumes only the terminal result. The chat host accepts only
declared previews, shares acknowledgement and cancellation with text previews, and admits terminal
alternatives before preview validation.

## Execution observations

`mf-chat` projects execution from the existing records used by the execution panel. A snapshot containing
public ToolCall `id` and `name` binds that identity to its run and exact runtime scope without retaining
arguments. Ordinary Flow calls use their existing CallStarted/CallFinished boundaries. Inside Iteration,
node records identify the unique direct child scope and its invoker from runtime ancestry, not receipt
order or overlapping timestamps. A direct body failure remains failed even when the invoker packs a
business-error result.

Native chat subscribes to existing lifecycle and Iteration records plus a SnapshotRecorder. The compiled receiver feeds the same projection after decoding and admission, using the same Iteration path reconstruction as the execution panel. No Router configuration, dedicated execution event, exporter, model stream, storage schema, or dispatch step participates.

Preview identity, scope binding, execution lifecycle, canonical transcript, and observation closure are
independent inputs. The projection retains bounded pending evidence and joins by ID, scope, path, RunId,
and original sequence rather than arrival order. A snapshot without an actual child execution is not
listed; missing or conflicting evidence is unknown. Transcript results never establish success. Only
explicit caller or runner interruption turns an unfinished started call into Interrupted; ordinary
closure without a terminal remains unknown.

## First output timing

The first nonempty text or tool-call delta marks first output before publication or backpressure. Identity and name hints count as tool-call output. The existing request start and finish facts determine the model timing window; no raw delta or tool execution is included in accounting. The reply footer and graph card show FTT without a call coverage fraction.

## Presentation

The live cache groups arguments by invocation, model call, and index. Rendering and keyboard
navigation use the same live tool group projection. Each group automatically expands when its
arguments begin streaming and collapses when turn settlement replaces the preview. Its arguments
and any result remain behind a leading `[+]` or `[-]` marker. Tool execution status does not control
folding. Preview rows use the model-call identity to reproduce the
same message and inter-tool spacing as settled batches. Unmatched observations use the same rule
without guessing a message identity. Browse-mode j/k selects groups in display order, with
Tab/Shift+Tab retained as aliases and arrow keys reserved for scrolling. Space or Ctrl+T toggles
the selection. A late, unambiguous wire ID transfers focus, expansion, and
scrolling to the canonical group key. Only a correlated execution observation updates the group
status. Missing or ambiguous hints do not match by position or name.

Authoritative messages replace previews without duplicating a call. If both are visible on the
current history page, results appear under their matching calls within the same turn and assistant
batch. A result-only page keeps the ID and the same fold key. Grouping does not reorder stored
messages. Raw argument previews and canonical arguments share incremental JSON syntax highlighting.
Incomplete or malformed fragments keep their exact text without waiting for JSON validation.
The terminal retains all received text and tool arguments for the active turn, with no entry or
byte cap. The terminal captures validated runner deltas before the invocation returns: ordinary
subscription receipts can arrive after settlement. It commits each available batch to the session
database before publishing the frame. Separate turn-scoped segment headers and ordered chunks
preserve immutable prefixes, late identity hints, and first-seen order. The terminal publishes
these committed prefixes only upon receiving their corresponding application Flow deliveries; runner
output never bypasses the graph to reach the view. A read view pinned just before settlement keeps
the final frame readable without holding a transaction during provider I/O. An interrupted turn reopens with
its drafts, independently of later turns. Settlement deletes replaced segments in the same
transaction as the authoritative result; write rollback leaves both unchanged. The private scratch
database stages only complete unsaved reply bodies for display, not streamed drafts.
Committed replies and history pages index headers and read canonical fields on demand. The renderer
keeps scroll anchors and measured heights but evicts offscreen body documents and their
wrapping/highlighting caches. Returning to a body loads its full text again. Neither drafts nor fold
state enter audit messages or model context. Full arguments
remain in the authoritative messages and in actual dispatch. Arguments and results remain literal;
assistant text keeps Markdown rendering.
