# Embedded chat core

`mf-chat` owns SQLite storage and application-level conversation orchestration. Direct database APIs
and application-owned `mf_chat.*` nodes use the same storage handlers. The library does not initialize
a terminal or launch workflow runner processes.

A conversation has two independent views:

- **Conversation transcript:** append-only original submissions and generated assistant/tool records,
  used for audit and terminal presentation.
- **Model context:** a revisioned continuation selected by the user Flow. It can append a normal
  exchange or replace the complete context without editing any displayed history.

This is an incompatible chatflow protocol update. Update and recompile older chat executables; there
is no fallback from scalar `text` results or separate `prompt`/`system`/`history` conversation inputs.
Existing databases receive forward migrations that preserve historical messages and accounting.

## Database initialization

Open an explicit database path:

```rust
let database = mf_chat::ChatDatabase::open("state/chat.sqlite")?;
println!("{}", database.path().display());
```

The owner creates parent directories, acquires an exclusive sibling lock, applies embedded pending
migrations, and keeps the lock until dropped. Another owning host receives a typed busy error.
Initialization enables WAL, foreign keys, and a finite busy timeout. No external migrations or tools
are needed. Preparation validates node contracts without opening storage; `ChatError` retains typed
filesystem, connection, query, migration, and transcript-validation sources.

Records distinguish sessions, ordered messages, turns, application invocations, and reported model
calls. One invocation can report several Agent, compaction, and title calls without treating tools or
summary messages as extra model attempts. Schema rollback is a development operation on disposable
databases; a downgrade that cannot represent tool records or replacement checkpoints is rejected.

## Sessions and history

Database APIs invoke storage handlers directly, without executing workflows:

```rust
let session = database.create_session("default", Some("Answer briefly".into()), None)?;
database.rename_session(&session.id, "Project questions")?;
database.archive_session(&session.id)?;
database.restore_session(&session.id)?;
let sessions = database.list_sessions(false, 50, None)?;
let transcript = database.messages(&session.id, 50, None)?;
let model_context = database.model_context(&session.id)?;
```

Lists use bounded keyset pages. Pass `next_after` back as the cursor. Session order is creation time
followed by stable ID; message order uses a checked per-session sequence. A turn can have any number
of assistant and tool messages, including repeated roles. Message records carry turn state and
turn-level usage facts. Pages can begin or end inside a tool exchange.

`messages()` reads original conversation records, including unsuccessful submissions and valid
failed display prefixes. `model_context()` separately returns `{revision, messages}` for the currently
published continuation. It is not a bounded transcript page and does not reconstruct context from
all successful audit messages after a replacement.

A new session seeds its context from its initial system instructions. Migration seeds existing
sessions from those instructions and successful legacy exchanges exactly once. A replacement can
edit or remove system instructions; later requests do not reinsert `session.system`. Archiving keeps
all records but prevents new turns until restoration.

## Shared conversation contract

### Inputs

`ConversationInputs.messages` contains the admitted complete model context followed by exactly the
current user submission. A chatflow declares one unambiguous required `messages: List(Object)` startup
port. Its qualified name can belong to any startup node, such as `params.messages`. The host, not
terminal configuration, binds this input.

The optional application-owned `context: Object` startup port receives title policy, the admitted
model-context revision, and compaction policy:

```json
{
  "title": {
    "source": "provisional",
    "revision": 0,
    "attempted": false,
    "enabled": true
  },
  "model_context": {"revision": 3},
  "compaction": {
    "enabled": true,
    "context_window": 128000,
    "reserve_tokens": 16384,
    "keep_recent_tokens": 20000,
    "custom_instructions": ""
  }
}
```

A Flow need not declare context, model, endpoint, or credential ports to be a valid chatflow.
Remaining startup ports are ordinary workflow configuration. Resolve their defaults and environment
bindings before admission; caller configuration cannot override application-owned conversation inputs.
Stdin-dependent chat executables are rejected.

### Alternative terminal outputs

A Flow advertises `append`, `replace_context`, or both. Each selected output is a self-contained
result object with its own `status`, `usages`, optional `error`, optional `title`, and variant-specific
message lists. Output names are the wire tags: no `kind` field, `update` wrapper, or merge node is
required.

A normal result exports only newly generated additions:

```json
{
  "append": {
    "status": "succeeded",
    "messages": [
      {"role": "assistant", "content": "The answer is 42."}
    ],
    "usages": []
  }
}
```

The next context is the admitted request, including its current user, plus `append.messages`. Do not
echo the incoming request. A Flow can preprocess a private provider request, but returning `append`
still continues from the original admitted context; the host does not infer a rewrite.

A replacement supplies a complete continuation and independent original display additions:

```json
{
  "replace_context": {
    "status": "succeeded",
    "context_messages": [
      {"role": "system", "content": "Summary: the calculated result was 42."}
    ],
    "display_messages": [
      {"role": "assistant", "content": "The answer is 42."}
    ],
    "usages": []
  }
}
```

The host publishes `context_messages` exactly, including `[]`. It does not append the current user,
final assistant, or display delta to that snapshot. A snapshot may omit them, summarize current tool
output, reorder or edit context, or coherently rewrite tool-call IDs. The UI appends only
`display_messages` and never displays the replacement as chat history.

For direct conditional branches, use existing optional output selections:

```json
{
  "outputs": [
    {"name": "append", "node": "normal", "port": "value", "optional": true},
    {"name": "replace_context", "node": "compact", "port": "value", "optional": true}
  ]
}
```

Both selections must be optional when both are advertised. Exactly one must actually be produced
per invocation. Zero, both, repeated results, undeclared alternatives, null/nonobject payloads,
missing required facts/lists, and unknown payload fields fail validation. A later conflicting result
invalidates the earlier recovery candidate; the host must not save the first arrival. Empty lists
are values, not absent outputs. Manifests do not describe payload schemas, so capture decodes and
validates fields even after preflight succeeds.

Rust backends return `ConversationResult { status, update, usages, error, title }` with
`MessageUpdate::Append` or `MessageUpdate::ReplaceContext`. This enum is host-internal normalization,
not an author-side Flow protocol. Its native codecs and canonical JSON use the same named output
objects, including inside `Completion.result` and retained recovery state.

### Messages and validation

Both views reuse `mfn_llm::ChatMessage`, `ToolCall`, and `ToolResult`; there is no chat-specific message
schema. Roles are `system`, `user`, `assistant`, and `tool`. Only assistant messages carry
`tool_calls`; tool results carry `tool_call_id`. Arguments remain arbitrary JSON in records, although
Agent dispatch requires an object compatible with the selected tool interface.

```json
[
  {
    "role": "assistant",
    "content": "",
    "tool_calls": [
      {"id": "lookup-1", "name": "lookup", "arguments": {"key": "a"}},
      {"id": "lookup-2", "name": "lookup", "arguments": {"key": "b"}}
    ]
  },
  {"role": "tool", "tool_call_id": "lookup-2", "content": "B"},
  {"role": "tool", "tool_call_id": "lookup-1", "content": ""},
  {"role": "assistant", "content": "Finished checking."}
]
```

Assistant call batches may have empty content; tool results may also be empty. Parallel results can
arrive in either order but must resolve their matching unique pending IDs. Ordinary user/system
messages and assistants without calls need nonempty content. Unknown role-specific fields, duplicate
or orphan IDs, interleaved unresolved batches, and missing results in a complete view are rejected.

A successful result requires a complete display exchange ending with a nonempty assistant answer
without calls, and an absent or empty error. A failed result requires a nonblank error and no title;
its display delta can be empty or a valid partial prefix with unresolved calls only in the final
batch. It may retain earlier complete rounds. A replacement snapshot must always be complete,
even for a failed result. Display and snapshot validation are independent: the same exchange can
appear in both without being mistaken for duplicate calls across one transcript.

A failed result can therefore preserve original tool records for audit while publishing no context:

```json
{
  "append": {
    "status": "failed",
    "messages": [],
    "usages": [
      {"call_id": "request-1", "model": "example-model", "error": "Provider unavailable"}
    ],
    "error": "The final model request failed"
  }
}
```

Set `capture_errors: true` on LLM nodes that should report request failures as data. Configuration,
input, cancellation, and infrastructure errors can still abort before any terminal result. Previews
and observations are never substitutes for missing messages or usages.

## Queuing and steering

A **turn** runs from admission of one prompt to settlement. It has one `turn_id` and can include
several model requests and tool batches. While it runs, choose where new input belongs:

| Intent | Submit | Withdraw newest unclaimed | When the text reaches the model |
| --- | --- | --- | --- |
| New turn in the same session (all chatflows) | Alt+Enter | Alt+Up | After the preceding turn commits successfully, with its newly committed context |
| Steer this turn (opt-in Agent) | Alt+S | Alt+Down | In the same runner's next request, after a complete reply and its entire tool batch |

`Alt+Enter` starts a turn when idle. During a turn it **always queues**: the host keeps the prompt
in memory, without allocating a turn or writing a message until its turn starts. One successful
commit releases at most one queued prompt, in FIFO order. Failure, cancellation, or an unsaved
result stops the queue and returns unadmitted prompts for editing. Saving that result or reopening
the application does not restart the queue.

Only `Alt+S` steers. A normal text answer with pending steering also continues the same execution.
No tool is interrupted or replayed. A closed inbox rejects late steering without clearing the editor
or turning it into a follow-up. Withdrawing from either queue leaves the other queue, the active
turn and newer edits alone. An already claimed input cannot be withdrawn.

The standard Agent declares a finite `workflow.call` named `callback:on_reply`. The calculator,
context, and DeepSeek examples need no steering-specific startup ports, nodes or forwarding. The
host binds the unique declared site to the invocation's inbox. Embedded execution calls it directly;
a compiled runner uses the [host callback RPC](workflows.md#host-callback-rpc).
The Agent supplies only the position and outcome, not its transcript. With no host binding the
ordinary local body returns an empty list. Only the invoked conversation Flow is inspected; the
host's title and compaction calls and movable tool Flows do not inherit the binding. In a custom
chatflow, reserve `on_reply` for the main exchange. Multiple static sites of that name disable
steering rather than guessing which Agent should receive the input. Neither tools nor the model
receive the callable or its transport. `ChatJob::steer` returns an ID;
`job.steering().unwrap().withdraw(id)` returns the text only if withdrawal won the race with the
pull. A cloned inbox can also recover unsent steering after cancellation. Each queue accepts up to
32 pending messages totaling 256 KiB of UTF-8.

The host keeps delivery receipts in memory for the active call and any pending save-only recovery.
It checks exact text, order, and positions in the display delta. Steering does not read or write
SQLite at a reply boundary: user messages become durable only in the ordinary turn settlement.
A Flow cannot invent user input for the audit. A failed turn still publishes no model context. Cancellation or
failure does not deliver pending input; the TUI restores it beside any newer draft while open.
Late submissions are rejected without clearing the editor. Closing the application discards the
in-memory queue; reopening does not resume it.

## Turn lifecycle

The host-bound `begin` call atomically stores the original user, running turn and application invocation,
admitted context revision, and advertised alternatives when contract-backed. It reserves one user
sequence and loads the complete admitted context before releasing its short transaction. Only one
turn runs per session; other sessions can progress while a provider is pending.

`ChatDatabase::complete_turn` and the host-bound `finish` call share the same settlement logic. It
validates against that immutable admission, appends every display message with checked sequence
allocation, and saves the complete canonical result, usages, and terminal state together. Successful
settlement also publishes the derived context revision and applies an eligible title in that
transaction. Failed results save their valid display prefix and facts but publish neither context
nor title.

Context publication compares-and-swaps the admitted revision. A stale successful update fails
without partial messages, facts, or title writes; save-only retry cannot silently rebase it. Title
revision is independent, so a concurrent manual rename preserves its title without blocking valid
context publication. Append revisions reference the original exchange; replacement checkpoints
contain their exact full snapshots.

Replay compares the complete canonical result, including variant, both replacement lists, error,
title, metadata, and usages. Identical replay returns the stored completion before attempting CAS.
It never republishes an old context after later turns. Different data for the same identity conflicts.

`ChatDatabase::fail_turn` and `ChatDatabase::fail_call` finalize work without a terminal result and
cannot overwrite committed data. Operational error summaries are bounded; canonical result errors
remain complete. `ChatDatabase::turn` and `ChatDatabase::call` expose individual records for
inspection. Opening a database under its ownership lock marks abandoned running work interrupted,
without automatically resending requests or rebuilding context from previews.

## Embedded conversation host

`ChatHost` owns database access, profiles, admission limits, execution workers, and abort cleanup.
Each submission executes the application-owned conversation Flow (registered as the built-in
`mf-chat.conversation` Flow and inspectable with `mf flows mf-chat.conversation`):

```mermaid
flowchart LR
    Admit["Admit and load context"] --> Bind["Bind request"]
    Bind --> Invoke["Invoke user chatflow once"]
    Invoke --> Preview["Publish provisional previews"]
    Invoke --> Validate["Validate terminal result after backend completion"]
    Validate --> Title["Optional application title"]
    Title --> Compact["Optional application compaction"]
    Compact --> Settle["Settle atomically"]
```

The host supplies the executors for the `begin`, `workflow`, and `finish` calls in
[`conversation.json`](../crates/mf-chat/workflows/conversation.json). Finite and compatible
TaskStream graphs run as the `workflow` call's child, preserving their nested execution scopes.
Executable runners, custom backends, and embedded graphs with their own Event/Stream coordinator
use the same call contract. Its incremental output publishes previews, and its terminal outputs drive
settlement after the backend completes normally and the result is validated. The host drains declared
streams with or without a preview subscriber; subscriber delivery does not gate persistence.

CEL gates decide whether to call the title and compaction Flows using eligibility facts. Credentials
and transcripts travel over separate native ports. Load a chat profile through `ChatHost` or the
chat CLI to execute the conversation Flow with its host bindings.

The user Flow owns business generation, tools, explicit usage composition, and captured-failure
policy, and may also own compaction thresholds and summary requests by declaring `replace_context`.
The application does not insert Agent or tool branches, identify internal model nodes, or collect
hidden model usage. Resource management and abort cleanup remain host responsibilities outside the
Flow success graph. Save-only retries call storage directly, never the conversation Flow.

```rust
let profile = mf_chat::WorkflowProfile::from_path(
    "default",
    "examples/chat-title-flow.json",
    Default::default(),
)?;
let host = mf_chat::ChatHost::open("state/chat.sqlite", mf_chat::HostConfig::new(profile))?;
let session = host.database().create_session("default", Some("Answer briefly".into()), None)?;
let mut job = host.submit(&session.id, "Hello")?;
let identity = job.invocation().clone();
while !job.poll() {
    std::thread::sleep(std::time::Duration::from_millis(10));
}
let completion = job.join()?;
host.close()?;
```

`poll` is nonblocking; `join` waits and reports failed business status as an error after its audit
result is stored. `cancel` requests cooperative cancellation. Dropping a job cancels it; closing or
dropping the host cancels and joins workers before releasing storage ownership. The default host
admits four active workflows independently of the runtime domain limit. Busy sessions and capacity
limits produce explicit admission errors for `submit` and `retry`.

For normal start-or-queue behavior, use `host.submit_or_queue(session_id, prompt)` and match
`PromptSubmission::Started(job)` or `PromptSubmission::Queued(receipt)`. To queue **only** if that
session is active, use `host.queue_next(session_id, prompt)`; to withdraw, call
`host.withdraw_next(session_id, receipt.id)`. `receipt.wait()` yields the next `ChatJob` once claimed,
including its own optional steering inbox. The worker does not wait for you to poll or join either
job, and dropping an unread receipt does not cancel an accepted prompt. An unstarted receipt returns
`ChatError::NextTurnNotStarted { prompt }`; if the claimed prompt fails admission, `job.join()`
returns `ChatError::NextTurnAdmission` with the original text and error source. Neither is retried.

Profiles contain resolved configuration or an injected `ConversationBackend`. Environment bindings
resolve during profile construction; model-specific references remain user-Flow policy. An optional
observer receives application and nested user execution. Run the supplied two-turn host example with
model configuration and credentials available:

```sh
OPENAI_API_KEY=your-key nix develop --command cargo run -p mf-chat --example conversation -- state/chat.sqlite examples/chat-title-flow.json
```

## Session title summaries

Title composition is optional. When policy permits it and unique resolved `model`, `base_url`, and
`api_key` inputs exist, the host can call `mf-chat.chat-title`. An empty key supports keyless endpoints.
Missing or ambiguous provider inputs skip this composition rather than invalidating a chatflow.
Defaults, environment bindings, and secret markers are preserved.

The call is eligible only for a successful result and an eligible provisional title, without a
returned title or a reported title attempt. A branch's `title` is a candidate; usages attributed to
`title` also suppress duplicate work. A separate root `title` output is not an ownership hint and
cannot supply a missing branch field. `HostConfig::automatic_titles = false` disables application
composition. Custom backends can opt in through `builtin_title_parameters`.

The source is the admitted original user and final assistant in the **display delta**, never tool
text, preambles, or replacement context. Empty and summary-only snapshots therefore do not hide the
answer from title generation. Title-model messages are not added to either conversation view.
Conversation and title usages settle together; title provider failure preserves the primary result
and records the attempted call. There is no background title queue, implicit retry, or startup job scan.

The [title example](../examples/chat-title-flow.json) expresses its own policy using `context.title`.
The [nested title example](../examples/chat-nested-title-flow.json) calls the reusable title Flow and
combines usages. See [reusable Flows](workflows.md#reusable-flows) for that helper's separate interface.
Neither example needs a root title output.

The terminal uses the first prompt line as a provisional name. `create_provisional_session` provides
the same behavior programmatically; an explicit title passed to `create_session` is manual. Manual
names take precedence. Title candidates are trimmed, nonempty, free of control characters, and at
most 120 characters. Invalid or stale candidates do not prevent saving the main result. The original
candidate remains in the canonical result for replay; read the saved session for the applied title.

## Tool Agents and context replacement

The [calculator example](../examples/chat-calculator-flow.json) calls the fixed `mfn-llm.agent` Flow
with a complete `params.messages` request, `Map(Flow)` tools, and matching `Map(Object)` properties.
It supports addition, subtraction, multiplication, and division. A normal `builtin.object` assembles
Agent `status`, generated `messages`, `usages`, and `error` into the single `append` output.

The [DeepSeek web Agent](../examples/chat-deepseek-flow.json) uses the same contract with separate
Search and Fetch tool Flows. The model can issue parallel searches and fetch discovered pages in later
rounds. Its root parameters separate `llm_model`, `llm_base_url`, and `llm_api_key` from
`deepseek_base_url` and `deepseek_api_key`. Keys can come from `OPENAI_API_KEY` and `DEEPSEEK_API_KEY`
or explicit CLI arguments. Each tool Flow binds its DeepSeek settings with context templates in the
producing Flow constant's `config.inputs`; only unconfigured public inputs become model arguments.
No Agent-specific argument channel or hidden child environment lookup is required. Network-bound
Search and Fetch nodes also make the nested execution PiPs easier to observe than the calculator's
short local operations. See [configured Flow tools](workflows.md#configured-flow-tools) for setup,
provider charges, and latency limitations.

The Agent accumulates assistant call batches, ordered parallel tool results, and the terminal
assistant without echoing the admitted request. As long as the model requests tools, the Agent
continues without a fixed round limit. Cancellation and the workflow step budget still apply.
Captured model failures retain completed rounds and usages. `answer` remains an optional stream of
assistant text, including tool preambles; it does not carry a live tool lifecycle protocol. Catalog
projection runs once before the first model call. See [configured Flow tools](workflows.md#configured-flow-tools).

Checked tool-operation failures, such as a DeepSeek Fetch HTTP 404, are returned to the model as tool
results rather than interrupting the whole Agent. The failed exchange remains in the transcript;
any subsequent request is an explicit model decision, not a hidden provider retry. Runtime and
cancellation failures still stop execution.

The [context example](../examples/chat-context-flow.json) reuses that calculator chatflow and exposes
two independent optional result outputs, with no convergence node:

- Normally it forwards the calculator's self-contained `append` result, including captured failures.
- With `params.compact = true` after successful generation, it explicitly calls a summary model on
  the admitted context and this turn's generated tool exchange. `replace_context` returns only the
  summary system message as continuation and the untouched Agent additions as display messages.
- The replacement branch reports both Agent usages and the additional `compaction` usage. Failed,
  incomplete, or empty summaries produce a failed audit result with the original display records;
  they do not clear or replace active context and are not retried automatically.

```sh
mf compile examples/chat-context-flow.json --output target/chat-context
mf run ./target/chat-context --chat --params.model your-model --params.compact true
```

The flag is example business policy, not a host compaction scheduler. Real flows can use their own
thresholds or summary strategies. The example's CEL assembly inherits the ordinary Code nesting
limit of 16 levels without expression, serialized-size, or collection-entry caps. Configure a
different business pipeline when explicit transcript budgets or truncation policies are required.

### Automatic and reusable compaction (`mf-chat.chat-compact`)

`mf-chat` ships the built-in `mf-chat.chat-compact` Flow (`workflows/compact.json`) and supports
three compaction modes—mirroring session title generation:

1. **Host-managed built-in compaction:** When `HostConfig::automatic_compaction.enabled` is true
   (default: `context_window = None`, `reserve_tokens = 16384`, `keep_recent_tokens = 20000`,
   `custom_instructions = ""`), the chatflow only declares `append` (not `replace_context`), and
   unique resolved `model`, `base_url`, and `api_key` inputs exist, the host composes
   `mf-chat.chat-compact` inside `workflows/conversation.json` after optional title generation and
   before atomic settlement. When `context_window` is `None`, the host queries `GET {base_url}/models`
   through `mfn-llm` once per `(base_url, model)` (caching results and non-cancelled misses for the
   host or TUI session lifetime) and falls back to `DEFAULT_COMPACTION_CONTEXT_WINDOW` (`128000`)
   when the provider omits window limits or rejects `/models`. Setting `context_window = Some(n)`
   skips `/models` probing and enforces `n` directly. If compaction triggers, the host publishes
   the compacted context via `ReplaceContext` while preserving the original turn's display messages
   and merging `compaction` usages atomically. If compaction is skipped or fails, the primary
   `Append` result is preserved along with any captured compaction usage. Set
   `HostConfig::automatic_compaction.enabled = false` to disable host compaction; negative token
   settings fail validation when opening `ChatHost`.
2. **User chatflow reuse:** A chatflow can call `{"builtin": "mf-chat.chat-compact"}` directly,
   passing `params.context` straight through to inherit resolved policy defaults (including any
   API-discovered `context_window`) from `context.compaction`, querying `llm.model_info` directly,
   or overriding thresholds and models, and export `append` / `replace_context`. Declaring
   `replace_context` causes the host to yield automatically so compaction never runs twice.
3. **Custom compaction:** A chatflow declaring `replace_context` with custom summarization (such as
   [examples/chat-context-flow.json](../examples/chat-context-flow.json)), or a custom
   `ConversationBackend` that returns `MessageUpdate::ReplaceContext`, reports a usage with
   `attribution.purpose == "compaction"`, or overrides `builtin_compaction_parameters` to return
   `None`, suppresses host-managed compaction.

Internally `mf-chat.chat-compact` delegates compaction execution to `mf-chat.chat-compact-run`
(`workflows/compact-run.json`), which composes `mf-chat.chat-compact-plan`
(`workflows/compact-plan.json`) and the shared `mf-chat.chat-compact-summarize`
(`workflows/compact-summarize.json`) Flow:

- **Inputs:** `params.messages` (required), `params.append_messages` (`[]`), `params.usages` (`[]`),
  `params.context` (object carrying optional `compaction` policy fields: `enabled` `true`,
  `context_window` `128000`, `reserve_tokens` `16384`, `keep_recent_tokens` `20000`, and
  `custom_instructions` `""`), `params.force` (`false`), `params.model`, `params.base_url`, and
  `params.api_key`.
- **Outputs:** `status` (`"skipped"`, `"succeeded"`, or `"failed"`), `compacted` (`bool`),
  `messages`, `summary`, `usages` (attributed with `purpose: "compaction"`), `tokens_before`,
  `read_files`, `modified_files`, and `error`.
- **Policy:** Negative token settings fail explicitly. When `!enabled && !force`, the Flow takes a
  fast path that skips transcript scanning and preserves `messages` directly. Otherwise it estimates
  context tokens from the last valid non-auxiliary usage plus trailing messages (falling back to a
  character-based estimate), triggers when `force` is true or
  `tokens_before > context_window - reserve_tokens`, keeps leading system messages and recent turns
  without cutting on a tool result, handles split turns with a separate turn-prefix summary, truncates
  summarized tool results to 2,000 characters, and updates previous compaction summaries and
  cumulative `read`/`write`/`edit` file lists stored in `metadata.compaction`. Summarization failures
  or truncated replies keep the original messages unchanged and return captured usages and `error`.

## History metadata

Every message can carry opaque object-valued `metadata`, including nested values and nulls:

```json
{
  "role": "assistant",
  "content": "See the referenced document.",
  "metadata": {"citations": [{"uri": "https://example.com/reference"}], "note": null}
}
```

Metadata is omitted from provider requests. Display records keep their original content, call IDs,
arguments, result strings, and metadata through save-only retry, paging, and reopen. Context edits
change only the continuation copy. New user submissions currently have no metadata.

Native and JSON codecs share the `mfn-llm` field schema and reject duplicate JSON keys at external
byte boundaries. Serialization persists business values, including secret payloads; it is not a
redaction or encrypted-storage boundary. Native Flow capabilities cannot be persisted as message
arguments or metadata. No placeholder or alternative message schema is introduced for them.

## Usage statistics

Use `ChatDatabase::usage` or `ChatHost::usage`. Queries read committed facts in one SQLite snapshot
and never execute a model or change billing records:

```rust
let statistics = host.usage(mf_chat::UsageQuery {
    session_id: Some(session.id.clone()),
    purpose: Some(mf_chat::CallPurpose::Conversation),
    group_by: Some(mf_chat::UsageGrouping::Model),
    limit: 50,
    ..Default::default()
})?;
```

Each `usages` entry identifies one actual model attempt, including attempts without token counters.
Optional nonblank `attribution.purpose` and `attribution.profile` labels are independent of the owning
workflow profile and do not select providers or credentials. Missing labels fall back to invocation
purpose/profile. Explicit labels such as `title` and `compaction` preserve separate accounting.

Flows combine the exact lists they intend to report. Duplicate call IDs within a result are rejected.
The application does not infer attempts from tool messages, configured model hints, or observations.
An explicit empty list reports zero calls; an invocation with no terminal list remains unreported,
not confirmed zero. `unreported_invocations` exposes that uncertainty. Known legacy single-model
facts remain queryable; new writes leave scalar compatibility columns empty.

Filters accept session, profile, actual model name, purpose, and UTC Unix-millisecond start times;
start is inclusive and end exclusive. Group by session, model, profile, purpose, or UTC day. Group
pagination leaves the overall summary scoped to the complete filter. Every token counter exposes
`known_subtotal`, `reported_calls`, and `missing_calls`: absent is unknown, explicit zero is reported,
and partially reported subtotals are not complete totals. SQL aggregation detects integer overflow.

`mfn_llm::CallUsage` contains provider call facts. `mf_chat::CallUsage` adds event-derived
`timing.elapsed_us` and optional `timing.first_token_us` without changing their flat JSON shape.
The LLM emits `llm.request.started`, `llm.response.first_output`, and `llm.request.finished`; observation
stamps source Unix nanoseconds once. The first-output event precedes publication and backpressure for the
first nonempty text or tool-call delta; empty or metadata-only events do not count. The application
correlates call IDs and derives
microsecond differences. Executable chat receives the same events through OTLP before settlement.
Missing, conflicting, or noncausal sequences stay unknown; receiver arrival time is not substituted.
Raw standalone node outputs do not compute application timing.

`CallUsage::token_rates()` derives generation throughput from the saved output-token count divided
by duration after FTT, and end-to-end throughput from whole duration. These are client-observed rates,
including local backpressure, not server-only decoding measurements. Missing counters, failed calls,
and zero-duration windows remain unknown. Paired zero tokens yield zero over a positive duration.
Persistence retries preserve original timing rather than recalculating it.

Overall and group `performance` summaries expose elapsed/first-token totals and means, plus weighted
paired-token rates. Each includes reported/missing coverage. Aggregation sums paired tokens and
positive durations rather than averaging per-call rates. Monetary estimates need separate pricing.

A turn displays one dim footer, for example `FTT 500ms · Gen 66.7 | E2E 50.0 tok/s`. Narrow views
prioritize FTT and generation throughput. Multiple calls show mean FTT and weighted rates;
unknown values stay `—`. Conversation and unlabeled calls contribute; other explicit purposes do not.
Message pages attach required facts once per visible turn, including pages starting with a tool
result. Rendering and reload never multiply charges by message count. Retained results keep the same
facts. `UsageSummary::from_calls` shares persisted aggregation rules.

## Optional live answer subscriptions

`ChatHost::submit` and `retry` allocate no preview receiver. Use `submit_stream` or `retry_stream`
to subscribe before launch and read concurrently for live display:

```rust
let (job, events) = host.submit_stream(&session.id, "Hello")?;
let reader = std::thread::spawn(move || {
    while let Ok(event) = events.recv() {
        render_provisional(event);
    }
});
let completion = job.join()?;
reader.join().expect("reader panicked");
```

The public `answer` output contains `text` and `model_call_id`. It is selected through ordinary Flow
outputs independently of internal node kinds. An application `Invocation.call_id` is not a model
call ID; one turn can contain many model calls. A model can emit no fragments. Normal stream drain
and execution closure establish completion, not a special chat transport envelope.

The host acknowledges native deliveries after handling or discarding them. Channel subscriptions
queue a bounded number of previews in order and drop new previews while full. A slow or disconnected
subscriber does not block or cancel the turn, and `join` does not require draining the subscription.
Explicit cancellation and host shutdown still interrupt active work. Text prefixes alone never become
database messages or usage facts.

Tool-aware chatflows may select the shared Agent's optional `tool_call_delta` stream. Each event
contains a raw `arguments` fragment, its `model_call_id`, and the provider tool `index`, with optional
`id` and `name` hints. An identity-only event has an empty `arguments` string. A fragment need not be
valid JSON on its own.
`PreviewEvent.delta` distinguishes `PreviewDelta::Text` from `PreviewDelta::ToolCall`. Neither enters
history or model context. The calculator, context-replacement, and DeepSeek examples explicitly
select `tool_call_delta`. A chatflow can select it without exposing `answer`.

Terminal preview capture runs independently of drawing and inspection. Before publishing a frame,
the terminal commits the available text and raw argument fragments to `preview_segments` and
`preview_chunks` in `--chat-db`. These are ordinary tables keyed by turn, model call, and tool index,
not a second message history. Each batch appends only new bytes. A frame addresses exact prefixes
without copying the turn's bodies. There is no entry, byte, or argument-tail limit. If a draft write
fails, the terminal reports it and stops extending that preview; the runner can still finish and save
its result.

If the chat process dies, reopen the same database and select the conversation with `Ctrl+S`.
Previously displayed text and tool arguments appear under the interrupted turn as `Response
unfinished`. Tool groups reopen collapsed and can be expanded even when their arguments are not
valid JSON. Retrying or starting a later turn does not overwrite them. Recovery does not resume the
model or run a tool. Drafts never enter `messages`, model context, usage, or the save-only candidate.
A successful result replaces the whole draft; a failed result replaces the parts covered by its
display transcript and leaves the rest unfinished. Draft cleanup and result persistence commit or
roll back together. Protect the session database accordingly: it can now contain partial tool inputs
and output from unsuccessful turns as well as completed conversations.

## Executable conversation backend

`ChatContract` validates the external startup/result interface, not internal model counts, graph
layout, loops, or business policy. `mf run runner --chat` uses the frozen executable as the backend
of the application Flow's `workflow` call. Opening chat and submitting turns do not rebuild the
executable or load original Flow sources. Provider configuration travels through a private argument
file. The supervisor owns child execution, cancellation, and observation.

Finite executables keep terminal JSON. Native streaming executables use JSONL with optional `answer`
records and exactly one complete alternative result. Capture validates the whole invocation and
retains the candidate until normal closure. A later conflicting result invalidates recovery rather
than preserving a first-result winner. Unrelated workflow outputs do not supply missing result facts.
Result capture is bounded by `MAX_RECOVERY_BYTES` (16 MiB).

The [single-call](../examples/chat-flow.json), [title](../examples/chat-title-flow.json),
[loop](../examples/chat-loop-flow.json), [calculator](../examples/chat-calculator-flow.json), and
[context-replacement](../examples/chat-context-flow.json) examples compile directly. The single-call
example exports only its terminal result; use a Flow that exports `answer`, such as the title
example, for live text previews. The loop forwards `chat.delta` through
`config.stream_outputs.answer` while accumulating all generated assistants and usages. Private loop
context is distinct from its exported additions.

## Persistence and recovery

The host-bound `finish` call composes optional title and compaction facts and invokes shared
settlement. Host abort cleanup and save-only retries use those handlers directly, independently of a
cancelled execution context. Direct callers can begin an `Invocation`, bind its request with
`contract.arguments`, then pass typed data to `reconcile_result` or bounded JSON to
`reconcile_completion`.

Recovery uses the admitted revision, original user, and stored advertised alternatives. It needs no
current manifest, source files, or workflow execution. `CompletionState::Committed` means a terminal
result is durable, whether business status succeeded or failed. `Uncommitted` retains the complete
candidate and identities, including failed prefixes and both replacement lists. `save_recovered` or
`ChatHost::retry_completion` performs storage only. Failed candidates never publish context. A stale
successful candidate cannot rebase; repeated writes remain idempotent. Explicit abandon finalizes
unfinished work without publishing the candidate.

Fresh model retry is different: it creates new turn/application IDs, links `retry_of`, and uses the
current published context. It may execute tools again. Cancellation and storage rollback do not undo
external tool effects; neither fresh retries nor crash recovery promise exactly-once tool execution.
No operation implicitly resumes an Agent or reruns a summary/title stage. A crash before durable
settlement can lose a reply and completed-call facts; interrupted work remains unknown rather than
inventing a partial successful exchange.

## Executable chat terminal

Compile a conversation workflow and launch it:

```sh
mf compile examples/chat-calculator-flow.json --output target/chat-calculator
mf run ./target/chat-calculator --chat --chat-profile default --params.model your-model
```

Storage defaults to `~/Library/Application Support/miniflow/chat.sqlite` on macOS and
`$XDG_DATA_HOME/miniflow/chat.sqlite` on Linux, falling back to `~/.local/share/miniflow/chat.sqlite`
when the variable is unset, empty, or relative. Use `--chat-db <PATH>` to override it. Help displays
the resolved path without creating storage; unavailable user directories require an explicit path.
`--chat` implies `--tui`, and both flags can be passed together.

A new conversation is persisted only after its first submission. Resumed conversations keep their
published context; original records remain visible after compaction. Each tool result is shown under
its matching call on the current page, regardless of result arrival order. A page without the call
still shows the result with its tool-call ID. Grouping changes only the display, not stored order.
Tool arguments and results are inert literal data, never prompts, Markdown images, or commands.
A tool call group automatically expands (`[-]`) when its argument preview first arrives. An open
group displays the argument text received so far with incremental JSON syntax highlighting. The display
does not require the fragment to parse as JSON. Each group automatically collapses (`[+]`) when the
whole turn finishes and its authoritative transcript replaces the preview; the model may already
be executing the next model round while the traced tool status updates. Manually expanding a
settled group displays its complete canonical arguments. Reopening history shows groups collapsed
and reads canonical arguments from `messages` or an unfinished turn's raw prefixes from its draft;
fold state is not persisted. A history page loads message and tool headers and turn usage,
not message bodies. Scrolling or expanding a group loads its content from SQLite. The renderer
keeps the documents intersecting the viewport and releases offscreen bodies, highlighting, and
wrapped rows; returning to a group reloads it without truncation. It lays out one whole body at
a time, so a single large visible message can still take substantial memory. Header and scroll
metadata remain in memory. Save-only recovery retains its authoritative candidate independently.
Live assistant blocks use model-call IDs and the same spacing between messages
and tools as settled replies. The status on each tool header
comes from existing snapshots and Flow or Iteration lifecycle records, matched by public call ID and
exact runtime scope without Router-specific configuration. Native and compiled chats share the
invocation-local projection and tolerate record reordering. An observed call without a matching
preview appears under `Observed tools`. `ChatJob::tool_metrics()` exposes the in-process view by
call ID. Missing observations and canonical tool results never imply a start or success.

The fold marker leads the group header. Focus and expansion follow a group when a late ID makes it
identifiable. Tool Flow completion updates the group's status in place but does not prematurely
collapse the streaming group. Results appear inside the group when the turn settles; execution
metrics are not saved.

Assistant text supports Markdown in saved records, previews, and retained results: headings, emphasis,
lists, tasks, quotes, links, tables, and code blocks. Headings show explicit levels `#1` through `#6`.
Fenced blocks use their language for highlighting; unsupported languages, very long code lines, and
highlighting errors fall back to plain code. HTML, prompts, diagnostics, and tool results stay
literal; JSON highlighting styles tool arguments without interpreting or reformatting them.
Terminal controls are escaped without modifying stored content.

Unchanged Markdown blocks and wrapped lines are cached. Resize reflows without repeating highlighting;
streaming highlighting resumes from complete lines. Scrolling up anchors content, while scrolling
to the bottom resumes tail following. The input grows with content; Unicode editing and bracketed
paste are supported, with a 256 KiB editor limit.

Assistant Markdown images support local paths and `file:`, `http:`, and `https:` PNG/JPEG/WebP. Paths
resolve against the initial working directory. Loading occurs in the background on viewport entry,
with 8 MiB and 4-megapixel limits and bounded caching. A new execution or settlement refreshes images.
Ghostty/Kitty use the Kitty protocol with placeholders; other terminals use colored half-blocks.
Detection reads terminal environment and dimensions, never consumes keyboard input or probes stdout.
Images fit within 120 columns and 40 rows, clip while scrolling, hide behind popups, and release
resources on session change, eviction, or exit.

| Interaction | Key |
| --- | --- |
| Send now if idle, otherwise queue a new turn | Alt+Enter, or Esc then Enter |
| Steer the active turn (when supported) | Alt+S |
| Withdraw the newest pending next-turn / steering input | Alt+Up / Alt+Down |
| Insert a newline | Enter while editing |
| Browse messages / return to editing | Esc / i |
| Open commands | Ctrl+P |
| Select model (when exposed by chatflow) | Ctrl+L |
| Toggle execution panel | Ctrl+E |
| Select next / previous tool group while browsing | j / k, or Tab / Shift+Tab |
| Manually expand / collapse selected tool group | Space while browsing, or Ctrl+T |
| New chat / switch saved chat | Ctrl+N / Ctrl+S |
| Cancel current execution | Ctrl+C |
| Quit after cleanup | Ctrl+Q |

Pickers use arrows and Enter; Esc closes them. For chatflows exposing unambiguous provider startup
parameters (`model`/`base_url` or `llm_model`/`llm_base_url`, plus optional `api_key`/`llm_api_key`),
the header displays the active model, `Ctrl+L` opens an incremental-filter model selector backed by
the `llm.list_models` node (`GET {base_url}/models`), and non-secret startup parameters plus the selected model are persisted per
`--chat-profile` in a sibling `<chat-db>.preferences.json` file (mode `0600` on Unix; secret
parameters such as `api_key` are never persisted). Subsequent launches reuse saved non-secret
parameters when omitted from the CLI; explicit CLI arguments always override and update saved
preferences. If no model is configured by CLI arguments, saved preferences, or an environment
binding (or if the CLI switches `base_url` without specifying a model), startup queries `/models`
once and defaults to the first model returned by the provider. Save/discard actions operate on the retained full
result without running generation. Unsaved display additions remain visible across user-only reloads;
context replacement is marked pending, not published. Failed records stay visibly unsuccessful,
including failed results with tool records or no assistant answer. Saving a failed audit result does
not publish its snapshot. A retry action starts a fresh execution and may repeat tool effects.

Session switching waits for active or retained work to be resolved. Pending input never owns a turn slot.
The controller reports `NextTurnClaimed` with the entry ID and new invocation when the predecessor
commits; `Submitted` follows only if durable admission succeeds.
Drafts survive idle switches. History opens at a bounded latest page; Earlier/Latest
commands page original records, not model context. Turn-level performance appears once per page and
is not duplicated on reload.

The Flow panel is enabled by default and uses the same default summary view as `--tui` and Inspect.
Wide terminals place it beside the transcript, narrower tall terminals below, and small terminals
show a status line. Ctrl+E does not stop observation and its preference survives new turns/sessions
for the application run. Parallel work uses the shared local picture-in-picture graphs, including
members inside folded calls. The last run remains visible until a new turn or session; Inspect opens
graph, trace, log, and data views and Esc returns to chat.

The terminal has no rename, archive, usage-dashboard, automatic-title, or parallel-execution controls;
those remain programmatic APIs or Flow policy. Storage and supervision run on a worker so drawing
and cancellation stay responsive. Quit cancels and reaps children, reconciles retained work, releases
ownership, and never prints turn JSON to stdout. Handled turn failures allow normal exit; display or
cleanup failures return status 1. Unsaved results are memory-only and are not resent after restart.
