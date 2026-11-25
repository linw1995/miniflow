# Message-based chatflow protocol

## Status and scope

Implementation and repository validation are complete. The message protocol, durable context,
Agent exports, execution adapters, and UI share one canonical contract. Acceptance covers embedded
and source-free compiled continuation, independent branch outputs, explicit summary-model accounting,
invocation-wide result invalidation, and save-only recovery.

A chatflow owns its Agent loop, tools, failure policy, and explicit accounting. `mf-chat` owns
admission, durable state, recovery, and presentation. Separate the append-only conversation
transcript used by the UI from the model context used for the next invocation. Updating model
context must never overwrite displayed history, original ToolCalls, or accounting.

## Pre-change constraints

These constraints describe the implementation before this change, not the new public contract.

- `crates/mf-chat/src/contract.rs` binds `prompt` plus optional `system`, `history`, and `context`.
  `ConversationResult` accepts one successful `text` and cannot return generated messages.
- `crates/mf-chat/src/turn.rs` reserves two message positions and inserts one assistant on success.
  Its failure path retains accounting and a summary, but discards generated messages.
- The `messages` table permits only user/assistant roles, nonempty content, and one message per
  turn/role. `session::history` reconstructs those two roles from scalar fields.
- `crates/builtin-nodes/llm/src/lib.rs` already defines role-tagged `ChatMessage` variants for
  system, user, assistant with `tool_calls`, and tool with `tool_call_id`.
- `crates/builtin-nodes/llm/workflows/agent.json` keeps tool exchanges inside its Loop and exports
  text/status/usages. Its final assistant is not appended to the Loop transcript.
- `mf-tui` treats every stored non-assistant message as a user and keys assistants by turn rather
  than message. Persisting multiple roles requires presentation changes as well as storage changes.

## Responsibility boundaries

The embedded conversation Flow is an application lifecycle coordinator, not an Agent or a hidden
answer-generation pipeline. Its normal path is conceptually:

```text
AdmitAndLoadContext
  -> BindRequest
  -> InvokeUserChatflow
  -> ValidateResultAndAwaitDeliveryClosure
  -> OptionalApplicationTitle
  -> SettleAtomically
```

These stages describe responsibilities, not a required new set of JSON node kinds. Existing
preparation, Call, routing, and finishing adapters may combine stages. Stream previews travel
through the application graph but cannot independently enter settlement.

| Owner | Responsibility |
| --- | --- |
| mf-chat conversation Flow | Orchestrate admission, request binding, one selected backend invocation, preview/final routing, protocol validation, optional title composition, and settlement. |
| User chatflow | Generate the business answer, choose models and tools, own Agent loops and explicit retries, decide whether/when/how to compact, and report one complete result with explicit usages. |
| mfn-llm Agent/LLM Flows | Supply reusable generation/tool mechanics selected by the user Flow; do not own database state or application settlement. |
| mf-chat chat-title Flow | Optionally produce a title candidate and its own usages; never rewrite either message view. |
| Host and backend adapters | Own profile preparation, configuration resolution, admission limits, workers, process supervision, cancellation, shutdown, and recovery after graph abortion. |
| Shared storage handlers | Enforce invocation ownership, transactions, revision checks, exact replay, transcript append, and model-context publication for both direct APIs and node adapters. |
| UI | Render original conversation records and provisional display data; never treat model-context snapshots as display history. |

### What the conversation Flow coordinates

Admission records the original user and stable invocation identity, pins the model-context revision
and advertised result contract, and constructs the request from that context plus the current user.
It must not reload the entire display journal as model input after a compacted context was saved.
Resolved configuration and application context are bound through the public interface, not by finding
or rewriting internal LLM nodes. The user chatflow does not need database handles or mf-chat nodes.

After invocation, the application validates the selected result, including exactly-one selection,
message/tool structure, and reported usages. It waits for normal backend closure and required preview
delivery before normal settlement. It interprets append versus replacement by the agreed protocol;
it does not decide that compaction is necessary or generate the replacement itself.

On success, shared handlers append only display additions and publish the chosen model context with
facts, title, and terminal state atomically. A valid failed result is an ordinary settlement input:
retain its reported transcript/facts for inspection, but do not publish model context or apply a title.
Protocol validation and SQL mechanics belong to their shared implementations, not duplicated JSON
policy or a second persistence path hidden in the host.

### Optional title composition

Retain chat-title as a separate optional application capability. The conversation may compose it
only when host policy enables it, resolved title-call inputs are available, the primary result is
successful, title eligibility permits an attempt, and the user Flow has not already supplied a title
or reported a title attempt. Public provider inputs may be reused as a convenience, but model/base_url/
api_key ports are not mandatory chatflow inputs and internal model nodes must not be inspected.

The title source is the original admitted user plus the final display answer, never the replacement
snapshot. The title Flow returns metadata and explicit usages, not conversation messages or context.
Captured title failure preserves the business result and records the title attempt; a concurrent
manual rename wins in storage. Do not create background title jobs or retry title stages implicitly.
Remove reliance on a separate root title output as an ownership hint now that candidates live inside
branch results; explicit host policy and returned title/call facts govern optional composition.

### What remains outside the success graph

Database initialization/locking, profile validation, worker limits, process reaping, and host teardown
are host responsibilities. Provider exceptions, cancellation, and crashes may prevent all finishing
nodes from running. The host must therefore finalize or reconcile admitted work through direct shared
handlers independently of the cancelled graph, preserving typed errors and valid retained results.
Save-only retry invokes storage, not the conversation Flow, user chatflow, tools, or title generation.
It cannot turn an ambiguous result into a valid candidate or guess missing intermediate call facts.
The host must not infer business usages from internal nodes or telemetry; observations may enrich
timing for explicitly reported calls without inventing attempts or counters.

## 1. Two durable views and one request

| View | Purpose | Write policy |
| --- | --- | --- |
| Conversation transcript | UI history, original submissions and tool exchanges, audit. | Append only; context replacement cannot edit it. |
| Model context | Messages retained for future chatflow invocations. | Explicit append or complete replacement on successful settlement. |

`messages(...)` reads the conversation transcript with bounded pagination. `model_context(...)`
reads the effective context and its revision. The UI must not load conversation history through the
model-context query. Reopening preserves both views independently.

Let `C` be admitted model context, `U` the newly admitted user message, and `P = C + [U]`.

| Workflow input | Type | Required | Meaning |
| --- | --- | --- | --- |
| `messages` | `List(Object)` / `Vec<ChatMessage>` | Yes | Complete request `P`, with the current user last. |
| `context` | `Object` / `ConversationContext` | No | Application policy, title state, and model-context revision. |

`context.model_context.revision`, when declared, exposes the admitted revision for inspection.
Storage owns the authoritative revision even for workflows without context. Configuration inputs
such as model, credentials, and callable tools remain separate; callers cannot override conversation
inputs as configuration. Startup names must be unambiguous and type-compatible, regardless of node kind.

`ChatHost::submit(session_id, prompt)` can remain a text convenience API. Admission durably records
`U` once; it does not publish running input to `C`. New sessions seed context revision zero with
nonempty system instructions, if supplied. Later requests use saved context rather than re-prepending
legacy system instructions. An explicit context replacement can therefore change or remove them.

## 2. Terminal branch outputs and internal update variants

| Workflow output | Type | Meaning |
| --- | --- | --- |
| `append` | `Object` | Complete incremental result: status, messages, usages, optional error/title. |
| `replace_context` | `Object` | Complete adjusted result: status, context_messages, display_messages, usages, optional error/title. |

Common facts belong inside the selected branch payload, not separate required workflow outputs.
Each branch is a self-contained result, so different statuses/usages do not require a second
convergence path. Status is succeeded or failed; usages explicitly reports model attempts; a failed
payload requires a nonblank error and forbids a title. Both update kinds retain the same facts.

A Flow may declare only append, only replace_context, or both. It must declare at least one and
produce exactly one result alternative per invocation. If both are declared, both output selections
are optional so that normal branch skipping omits the inactive alternative. A single append-only
Flow selects its ordinary incremental result as required append: no router, kind-tag wrapper,
unused compact branch, or convergence node is needed. Assembling that branch's own message and
usage fields into a result object is not convergence between alternative branches.

The output name itself is the external enum tag. The host normalizes it into one application type:

```rust
pub enum MessageUpdate {
    Append {
        messages: Vec<ChatMessage>,
    },
    ReplaceContext {
        context_messages: Vec<ChatMessage>,
        display_messages: Vec<ChatMessage>,
    },
}
```

This internal enum does not imply a public update port. The application codec reads common facts
from the selected result object and maps its messages or context/display lists into MessageUpdate.
Do not duplicate the output-name tag in a kind field or force authors to merge alternatives. The
append messages list and both replacement lists are required within their result objects,
even when empty. Missing alternatives, both alternatives, null payloads, undeclared alternatives,
malformed lists, and unknown replacement fields are protocol errors, not fallback opportunities.
Canonical result serialization uses the same branch-output form for execution and recovery; all
common facts stay inside the selected payload rather than being mixed from separate outputs.

Status is independent of variant selection: it decides whether candidate context may be published.
A failed result still selects one alternative and may retain a valid partial display transcript or
candidate snapshot for audit/recovery, without publishing context. Admission retains the advertised
alternative set for contract-backed execution so recovery can enforce it without reopening a manifest.

Remove the mandatory tagged update object and legacy text/messages/history/next-state result aliases.
Message metadata stays on each message. Generic LLM text/finish-reason projections remain available
outside this chatflow contract. Common status/usages/error/title fields retain their meaning.

### Ordinary incremental result

```json
{
  "append": {
    "status": "succeeded",
    "messages": [
      {
        "role": "assistant",
        "content": "I will calculate that.",
        "tool_calls": [
          {"id": "tool-001", "name": "calculator", "arguments": {"left": 2, "right": 3}}
        ]
      },
      {"role": "tool", "tool_call_id": "tool-001", "content": "5"},
      {"role": "assistant", "content": "The answer is 5."}
    ],
    "usages": [
      {"call_id": "model-001", "model": "example-model"},
      {"call_id": "model-002", "model": "example-model"}
    ]
  }
}
```

An append-only result carries only generated message additions, not a full context snapshot or
input echo. An Agent retains preambles, tool-call batches, ordered results, and its final answer.

### Context adjustment with separate display additions

```json
{
  "replace_context": {
    "status": "succeeded",
    "context_messages": [
      {"role": "system", "content": "Answer briefly."},
      {"role": "user", "content": "Summary: the project uses PostgreSQL and Rust."},
      {"role": "assistant", "content": "The answer is 5."}
    ],
    "display_messages": [
      {"role": "assistant", "content": "The answer is 5."}
    ],
    "usages": []
  }
}
```

The second example shows wire shape, not actual model work. A Flow invoking answer or summary models
must report their attempts explicitly. context_messages is complete continuation state after this
turn, not an edited old prefix. It may compact old or current tool output, omit the user, or omit the
final answer. display_messages retains this turn's original generated messages. The lists need not
share a prefix, suffix, length, or values. Only the display list creates conversation records; a
context-only summary is not a visible message or new tool execution.

## 3. Application rules

Derive a display delta `D` and candidate context `N` by matching the enum:

```text
Append(messages):
    D = messages
    N = P + messages

ReplaceContext(context_messages, display_messages):
    D = display_messages
    N = context_messages
```

Admission has already appended `U` to the UI/audit transcript. Settlement appends `D` once in both
variants. Successful settlement publishes `N`; failed settlement stores its audit result without
changing active context. Cancellation, abandon, and incomplete previews cannot publish context.

Never append `P`, `U`, or `D` to a `ReplaceContext` snapshot. An explicit empty `context_messages`
clears all model context while preserving the UI transcript, including this turn. The next request
then contains only the next submitted user message unless the Flow adds other request inputs itself.
With `Append`, the current user is included once through `P`, not echoed in the returned delta.

The host does not infer a diff or decide which messages a summary replaces. A private model-request
transformation is not durable unless the Flow returns `ReplaceContext`. No patch addressing or
automatic merge is required. Summary policy and its model usages remain owned by the Flow.

`ToolCall.id` identifies a tool invocation; `CallUsage.call_id` and `TextDelta.model_call_id` identify
model attempts. Neither is the application's `Invocation.call_id`. Messages need not correspond
one-to-one to usages; title calls create no conversation record, and context summaries may be synthetic.

## 4. Shared validation

Reuse strict role-tagged `mfn-llm::ChatMessage`, `ToolCall`, and tool results. Extract provider-independent
transcript validation in `mfn-llm`, with indexed typed sources shared by model requests and settlement.
Validation must not construct provider requests or acquire transport resources. Application enum,
status, and publication rules remain in `mf-chat`, not the runtime or LLM provider.

Common message rules:

- System/user content is nonempty. An assistant needs nonempty text or at least one tool call;
  tool results may have empty content. Role-specific fields and optional object metadata stay strict.
- Tool IDs are nonblank and unique in each validated transcript. Names retain existing LLM rules;
  parsed arguments are preserved without a new object-only restriction on historical calls.
- A result consumes one pending call ID. Unknown/duplicate results fail, and all pending calls must
  be answered before another non-tool message. Partial-prefix mode permits only a final unresolved batch.
- `D` contains generated assistant/tool messages and exactly the steering delivered to this invocation. It must not echo the initial user or old history.
  The owning host retains delivery positions and text in memory until settlement or abandonment, not in a separate database record.
  On success it has complete tool exchanges and ends with a nonempty assistant answer without calls.
  On failure it may be empty or a valid partial prefix; failure requires an error and forbids a title.
- `Append` validates `P + D` under the appropriate complete/partial rules, including collisions with
  visible historical call IDs. `ReplaceContext` validates `D` independently and validates the entire
  `context_messages` as a complete continuation transcript, permitting an empty snapshot.
- A replacement snapshot may use any supported role and coherently edit, delete, reorder, or summarize
  context. It need not retain `U` or `D`. Never concatenate the display and context lists for validation;
  the same call may intentionally occur once in each view.

Tool-ID uniqueness is not global across discarded context or all audit records. Scope audit correlation
to the originating invocation. Reject orphan results and collisions within a snapshot, but do not
reject a valid compacted snapshot because a removed exchange reserved an ID in old audit history.
The host never silently renumbers calls, invents results, or repairs invalid context.

Decode result shape without a manifest. Where needed, recovery loads the admitted immutable context
revision and original user, not the current context or all successful audit messages. Check a saved
result replay before comparing active context. Outstanding calls for an external executor would need
a separate `requires_action` lifecycle and are not successful completed chat turns in this change.

## 5. Storage, atomicity, and recovery

Store each conversation message with its existing storage ID, session/turn identity, sequence,
creation time, and one canonical message payload. These records are append-only regardless of update
kind. Remove fixed two-message reservation and `UNIQUE(turn_id, role)`. Admission allocates only its
user record; settlement allocates `len(D)` more positions using checked arithmetic. Context-snapshot
length does not affect transcript sequences, pagination, or message counts.

Store effective model context as immutable revisions with one active revision per session. A revision
records its parent, originating turn, and explicit operation:

- Append revisions reference that successful turn's admitted user and `D`, extending their parent.
- Replace revisions store the complete `context_messages` checkpoint and do not append the turn's
  display messages to it. An empty checkpoint remains a real replacement.

Materialize from the nearest initial/replacement checkpoint and subsequent append references. Do not
duplicate full context for ordinary turns or reconstruct removed context from the audit journal.
Keep admitted revisions available for recovery; pruning and general branching are outside this change.

Admission also retains the allowed result alternatives when supplied by a contract; direct custom
backends may support both. Recovery validates this stored admission contract without a manifest.
Persist the exact terminal result, selected alternative, all payloads, usages, and full error for replay. Bounded
error columns remain summaries. One immediate transaction writes `D`, result/accounting, turn/call
state, optional title, and context publication on success. Derive the title from original `U` and the
last assistant in `D`, never from a replacement snapshot that might contain only a summary or be empty.

Successful publication compares the active revision with the stored admitted base. A mismatch rejects
the whole settlement without partial records or automatic rebasing. Retain the result for explicit
resolution; retrying storage cannot fix a stale base by rerunning tools. Failed-result persistence
can save its audit/usages against the original admission without publishing or overwriting newer context.
Title-only renames do not change context revision. The revision is a context-state token, not the
removed derived successful-turn counter `conversation_revision`.

Idempotency compares the complete canonical result, including update kind and both replacement lists.
Changing a variant is a conflict even if it would produce equivalent context. Check equivalent saved
replay before the active-revision comparison: replaying an older settlement must return its saved
result without republishing old context after later turns. Committed success wins over late errors.

Retained save-only state supports successful and failed results. A storage failure preserves the whole
result, not just final text or usage. Retry never invokes models, tools, or summary generation. Without
a terminal result, cleanup retains only durable admission and marks missing usages unknown; previews
are not recovered as messages. Tool side effects are not rolled back by SQLite or deduplicated by a
fresh model retry.

Forward migration preserves message IDs, sequence gaps, metadata, titles, accounting, and ownership.
Seed model context from nonempty initial system instructions and successful legacy exchanges once.
Backfill known results as append updates when possible; do not invent old tool exchanges, unknown
attempts, or complete results for abandoned work. Existing executables must be updated and recompiled.

## 6. Flow and backend integration

All backends share ConversationInputs, normalized ConversationResult, and MessageUpdate. The
application's result codec maps branch output names to the internal enum and encodes only its active
alternative back to public outputs. Internal enum fields must not dictate a required Flow update port.
Use that same codec for canonical JSON recovery, nested application values, and registered runners.

ChatContract requires messages input and at least one advertised result alternative. Accept
append-only, replace-only, and two-output contracts without inspecting internal topology. Both
alternatives must be optional when both are declared. Both payloads are Object-valued; accept
compatible prepared map descriptors. Manifest-only preflight checks names/optionality because
existing executable manifests do not freeze output types. Required status/usages and variant fields
are checked inside each selected payload at decode. Never reconstruct a result by mixing facts from
other outputs. There is no adapter for old update wrappers, separate common outputs, or history fields.

Finite JSON and native JSONL still supply one complete terminal record: exactly one self-contained
result alternative. This does not require an authored convergence node; ordinary Flow
output selection collects selected terminal ports and omits explicitly skipped alternatives.
Embedded completion must wait for call success and read produced optional ports from context, not
bind both branches as ordinary data prerequisites that would skip settlement. Registered executors
must preserve declared optional/skipped ports and emit only the selected alternative. Do not
introduce incremental patch semantics or assemble separate partial terminal fields across records.

Exactly-one is invocation-scoped, not first-result-wins. Both alternatives in one terminal record,
a second terminal record in another domain, an undeclared alternative, or no result at closure must
fail before settlement. If a later record makes selection ambiguous, invalidate the earlier pending
candidate for save-only recovery as well; do not permit saving whichever branch arrived first.
An otherwise unique valid result followed only by process/delivery failure retains existing explicit
save-only recovery. Already committed success still wins over later conflicting reconciliation.

The generic built-in Agent consumes a complete request and returns generated assistant/tool
additions and host-delivered steering, including its final assistant. Tool catalog projection stays outside its Loop, and ordered
tool results survive parallel dispatch. The Agent continues while the model requests tools. Captured
model failures return available rounds/usages without inventing an assistant or claiming success.

An ordinary chatflow exports an incremental result object as append without an unused compact branch.
If the generic Agent exposes separate status/messages/usages ports, one ordinary object node can
assemble those fields; it is not an alternative-branch merge or an extra kind-tag envelope. The
compact branch constructs its own complete replace_context result, including its own usages and
status. A conditional Flow exports both directly. Generic Agent/LLM nodes need no mf-chat dependency.

Centralize variant matching for `D` and candidate context so database handlers, title preparation,
custom backends, runner registration, output decoding, and retained-result presentation agree.
The application Flow never inspects internal nodes to recover hidden tool output or infer compaction.

### Current definition-format feasibility

The current 2026-11-13 format supports branch-selected optional outputs without a new definition,
ValueType, or manifest version. Given a normal_result.value incremental result object, selection is:

```json
{
  "outputs": [
    {"name": "append", "node": "normal_result", "port": "value"}
  ]
}
```

A conditional Flow can instead select both branch producers:

```json
{
  "outputs": [
    {"name": "append", "node": "normal", "port": "value", "optional": true},
    {"name": "replace_context", "node": "compact", "port": "value", "optional": true}
  ]
}
```

These fragments omit surrounding definitions; common facts are inside each result. Gate normal/compact
with appropriate outputs of builtin.if_else; condition references still need explicit producer
prerequisites. Existing optional selection omits a skipped branch, while produced null remains data
and is invalid for either payload. The compiler need not prove protocol exclusivity across output
selections; mf-chat must enforce exactly-one over the invocation. An exclusive-input merge remains
available when an actual downstream node needs one value, but it is not required for chat result export.

Each branch may assemble its own result using builtin.object. The normal branch carries only a
message increment and common facts; the compact branch adds context/display lists. Their Map(Any)
outputs are compatible with Object and preserve native payloads/secret markers without CEL conversion.
ValueType has no closed union descriptor. Existing NodeEnum infrastructure remains
useful for role-tagged message values, but its kind-tagged codec does not itself select external
output names or enforce result cardinality. That application normalization needs implementation.

Local verification compiled append-only, replace-only, and two-output core-only Flows without any
convergence node and ran all paths. A separate fixture showed generic Flow output selection permits
zero or both alternatives, confirming that protocol cardinality belongs in the host decoder. These
checks verify current format/transport, not implementation of the new mf-chat admission, decoding,
persistence, or UI. The old single-update convergence construction was sufficient, not necessary.

## 7. Streaming and UI

Retain optional text-only `answer: TextDelta`, model-call identity, backpressure, cancellation, and
mandatory drain before normal settlement. It is provisional presentation, not a context update or
execution log. Live tool lifecycle events are deferred. A valid terminal result received before a
later delivery/process failure remains eligible for explicit save-only recovery.

Only `D` replaces the active turn's previews and enters committed or unsaved message presentation.
Never render `context_messages` as conversation history, including after reload or in retained state.
Keep original user/assistant/tool records visible after compaction; context-only summaries and edited
old tool data are not newly displayed messages. Context inspection is separate through `model_context()`.

Render assistant text as Markdown and tool data as inert structured/plain text. Use message-scoped
keys, not turn/role keys; pages can start inside an exchange and show tool IDs without guessing names.
Failed audit records remain visibly unsuccessful. Pending replacements must not appear as published.
Load usages/performance once per turn; context deletion cannot erase or reattribute paid attempts.

Keep presentation limits separate from authoritative capture. Existing 16 MiB recovery, private-input
transport, and Code conversion limits apply to complete payloads, including both replacement lists.
Do not silently truncate tool data or context to fit the display.

## 8. Delivery and acceptance

1. Shared codecs/validation, alternative output contracts, and enum normalization in the host.
2. Forward migration, separate revisioned context, arbitrary-length transcripts, and atomic recovery.
3. Agent transcript export, append/replacement examples, title preparation, and all backend adapters.
4. Display-only UI extraction, original transcript retention, and turn-level accounting.
5. Cross-backend fake-provider tests, documentation, and repository checks.

Cover two paths in embedded and compiled execution: ordinary append preserves a complete parallel
tool exchange for the next request; replacement compacts old and current tool output into a snapshot
while the original UI transcript and usages survive reopening unchanged. The next request must use
exactly the chosen context plus its new user, with no automatic snapshot append.

Also cover single-capability contracts, skipped alternatives, zero/both/undeclared outputs, a second
terminal record invalidating recovery, strict replacement fields, explicit empty snapshots, snapshots omitting current user or
final answer, display/context lists without a shared suffix, coherent tool-ID edits, orphan/collision
rejection per view, partial failures, failed/cancelled publication isolation, revision conflicts,
old replay after later turns, title generation from display data, and save-only retry with no backend
execution. Include the regression where the same complete tool exchange appears in both replacement
lists without duplicate-ID errors or duplicate UI/accounting records.
