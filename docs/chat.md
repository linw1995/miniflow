# Embedded chat core

The `mf-chat` library owns SQLite storage for multiple chat sessions. Direct database APIs and application-owned `mf_chat.*` nodes share the same storage handlers. It does not initialize a terminal or launch workflow runner processes.

## Database initialization

Open an explicit database path:

```rust
let database = mf_chat::ChatDatabase::open("state/chat.sqlite")?;
println!("{}", database.path().display());
```

The owner creates the parent directory, acquires an exclusive sibling lock, initializes storage directly, and keeps the lock until dropped. Another owning host receives a typed busy error. Startup applies embedded pending migrations and enables WAL, foreign keys, and a finite busy timeout. No external migration files or tools are needed.

Node preparation validates contracts without opening storage. Failures retain typed filesystem, connection, query, and migration sources through `ChatError`.

Persisted records distinguish sessions, ordered messages, conversation turns, and model calls. One conversation execution can report both conversation and title model calls without adding title messages. Schema rollback is an explicit development operation on disposable databases; startup only applies pending forward migrations.

## Sessions and history

Database APIs call storage handlers directly, without constructing or executing workflows.
They return storage variants of `ChatError` directly; callers can match domain errors without traversing runtime wrappers:

```rust
let session = database.create_session("default", Some("Answer briefly".into()), None)?;
database.rename_session(&session.id, "Project questions")?;
database.archive_session(&session.id)?;
database.restore_session(&session.id)?;
let page = database.list_sessions(false, 50, None)?;
let messages = database.messages(&session.id, 50, None)?;
let context = database.history(&session.id, mf_chat::DEFAULT_HISTORY_BYTES)?;
```

Lists use bounded keyset pages. Pass `next_after` back as the next cursor. Session order is creation time followed by stable ID; message order uses a per-session sequence. Message results include each turn's state, so unsuccessful submissions remain inspectable. Archiving retains messages and call records and prevents new turns until restoration.

History includes only completed successful turns from the requested session. It keeps system instructions separate and excludes unsuccessful submissions and the current prompt. The default canonical history JSON bound is 1 MiB; exceeding it fails explicitly without deleting or trimming messages.

Registered kinds are `mf_chat.session.create`, `.list`, `.load`, `.rename`, `.archive`, `.restore`, `mf_chat.messages.list`, and `mf_chat.history.load`. All node inputs name an explicit database path; node preparation never reads storage.

## Turn lifecycle

`mf_chat.turn.begin` atomically records a user message, running turn, and running model call. It reserves message order and loads successful history before releasing its short transaction. Only one conversation turn may run in a session; another session can still use the database while the provider is pending.

`mf_chat.turn.complete` saves the assistant message, reported usages, optional title candidate, and successful turn state together. It applies an eligible title in the same transaction. Replaying the same call and response returns the saved completion. Reusing its identity with different text, metadata, title, or usages fails. A failure halfway through completion leaves no assistant message or partial usage update.

`mf_chat.turn.fail` and `mf_chat.call.fail` finalize unsuccessful work and cannot overwrite committed success. Error summaries are bounded, while live errors retain typed sources. `mf_chat.turn.load` and `mf_chat.call.load` expose individual records for inspection.

A model retry creates a fresh turn and call and retains `retry_of` pointing to the original unsuccessful turn. A persistence retry reuses the original identity and response and never invokes the model again. Their database transitions are distinct.

Opening a database under its ownership lock calls the recovery handler directly: abandoned running work becomes interrupted and never automatically resends a request. A crash after a remote reply but before persistence can lose that reply and its usage. Interrupted usage remains unknown; it is not recorded as zero.

## Embedded conversation host

`ChatHost` owns the database, model profiles, and blocking execution workers. Each submission runs
the embedded `conversation` module with a fresh context and cancellation handle. The host admits the
turn before execution and saves the final `ConversationResult` afterward. The flow declares inputs
through `builtin.parameters` and uses only generic built-in nodes. Both execution backends bind
those ports through the same `ConversationInputs` and `ChatContract`; the flow has no `mf-chat` node
dependency. No standalone workflow runner is spawned.

```rust
let profile = mf_chat::ModelProfile::new("default", serde_json::json!({"model":"your-model"}))?;
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

`poll` is nonblocking, `join` waits for a result, and `cancel` requests cooperative cancellation. Dropping a job cancels that job; dropping or closing the host cancels and joins all its workers before releasing database ownership. The default host admits four active workflows, separately from the runtime's internal domain limit. A busy session or full host returns an explicit admission error.

Profiles contain validated nonsecret LLM configuration. Credentials are read through the configured environment reference only during execution. Sessions keep their profile and system instructions; each call records the profile and model actually selected. An optional observer receives each configured flow's preparation and execution lifecycle.

The flow owns failure policy. The default flow uses `capture_errors` to report provider failures as data; the host persists the returned status and usages. An execution error or cancellation with no final result finalizes begun work through storage handlers independently of the cancelled execution context. The host does not inspect internal node outputs, recover intermediate replies, or retry a failed stage.
If cleanup also fails, `ChatError::Execution` retains both typed errors and invocation identities. A reply whose storage write fails returns `ChatError::Uncommitted`; pass its retained value to `retry_completion` or explicitly abandon it. Repeated persistence failures return the retained reply again and never resend a model request. `retry` starts a fresh model call for an unsuccessful original turn.

Run the two-turn example with an available model and credentials in the environment:

```sh
MF_CHAT_MODEL=your-model nix develop --command cargo run -p mf-chat --example conversation -- state/chat.sqlite
```

Set `MF_CHAT_BASE_URL` for an OpenAI-compatible endpoint. The example prints complete results with their session, turn, and call identities.

## Session title summaries

The embedded conversation flow optionally generates a title after a successful primary model call. Its
`context.title` input contains the current title source, revision, whether an attempt has been recorded,
and whether automatic titles are enabled. The flow decides eligibility: provisional, enabled, and no prior
attempt. It uses the current prompt and answer as the title source. The host supplies the state snapshot;
workflow nodes do not read or write the database.

`HostConfig::title_profile` selects another configured profile; leaving it unset uses the conversation
profile. `automatic_titles = false` disables the default flow's title branch. Both calls report their
own purpose and profile through `usages[].attribution`. The default flow preserves a successful answer
when the title provider reports failure, retains that failed call's usage, and omits the title output.
This policy is expressed by the flow's branches, not a host-side exception handler.

A submission finishes after the composed flow finishes. There is no separate background title job,
startup scan, title retry API, or title completion queue. Cancellation before delivery can lose the
whole result and its usage; the host does not infer which internal calls completed. Persisted title
attempts, including failures, suppress subsequent automatic attempts. Unknown attempts cannot be
inferred from an interrupted workflow. Legacy never-started pending title jobs are retired on upgrade.

The terminal uses the first prompt line as a provisional display title. `create_provisional_session`
provides the same replaceable placeholder for other hosts; `create_session` with an explicit title still
means a manual name. Manual names always take precedence. Saving compares the admission-time title revision with the current
session revision. A rename during execution preserves the manual title while saving the answer and all
reported calls. Generated titles are trimmed, nonempty, free of control characters, and at most 120
characters. Invalid or stale candidates do not prevent saving the primary result. The original candidate
is retained for idempotent replay. A database failure rolls back the whole completion; `retry_completion`
or `save_recovered` saves the retained result without invoking either model again.

## Shared conversation contract

`mf_chat::contract` owns the common contract, argument binding, and result reconciliation for both
execution backends. `ConversationInputs` contains `prompt`, optional `system`, optional `history`, and
optional `context`. Context is application-owned input rather than a caller-supplied configuration
parameter. A workflow that does not declare context continues to receive only the inputs it supports.

```json
{
  "title": {
    "source": "provisional",
    "revision": 0,
    "attempted": false,
    "enabled": true
  }
}
```

`ConversationResult` contains `status`, `text`, `usages`, optional message `metadata`, `finish_reason`,
`error`, and optional `title`. `Completion.result` retains this result. A failed result requires an error
summary and cannot update the title. A successful result requires nonempty text. The title is a candidate,
not confirmation that a concurrent rename allowed it to be applied. Read the saved session for its title.

Context and optional outputs can later carry compaction state and a summary proposal. No compaction is
implemented: history loading still enforces the canonical byte bound before execution. A future compaction
stage also needs a history-loading policy and revision-checked summary persistence.

## Usage statistics

The former `flows::usage_statistics` module is removed; use `ChatDatabase::usage` or `ChatHost::usage`.
`usage` calls the query handler directly; `mf_chat.usage.query` exposes the same handler to workflows. It reads committed call facts in one SQLite snapshot and never invokes a model or changes billing records.

```rust
let statistics = host.usage(mf_chat::UsageQuery {
    session_id: Some(session.id.clone()),
    purpose: Some(mf_chat::CallPurpose::Conversation),
    group_by: Some(mf_chat::UsageGrouping::Model),
    limit: 50,
    ..Default::default()
})?;
```

Filters accept session, profile, actual model name, purpose, and call start times in UTC Unix milliseconds. Start is inclusive and end is exclusive. Group by session, model, profile, purpose, or UTC day. Pass `next_after` to fetch the next group page; the overall summary always covers the complete filter.

Each token counter exposes `known_subtotal`, `reported_calls`, and `missing_calls`. A missing observation is not zero; an explicit zero is a reported observation. No observations yield an unknown subtotal. Treat a subtotal with missing observations as partial coverage. Attempt and status counts include begun failed, cancelled, and interrupted calls, but exclude never-started title requests.

SQL computes aggregate facts without loading raw call history. Integer overflow fails explicitly. Calls from title generation remain attributed to `title`, including valid replies whose title update was stale or whose application title was rejected. Monetary estimates require separate pricing data and are not provided by the core.

## Executable conversation backend

`mf_chat::contract::ChatContract` validates a finite executable's external conversation contract. Internal node kinds,
node counts, control edges, preprocessing, and loops are workflow implementation choices. Execute the whole
workflow; there is no isolated-model execution mode or assumption that a workflow contains one LLM.

The startup interface supplies an unambiguous `prompt` string and optionally `system`, `history`, and `context`.
Their qualified names can belong to any startup node. Remaining declared inputs form `contract.parameters`;
resolve them with `mf_runtime::resolve_workflow_inputs` before admission. The contract rejects ambiguous or
mistyped conversation ports, stdin-dependent execution, and streaming output protocols.

Both the [single-call example](../examples/chat-flow.json) and the
[loop example](../examples/chat-loop-flow.json) can be compiled directly:

```sh
nix develop --command cargo run -p mf-cli --features development-support -- compile examples/chat-loop-flow.json --output target/chat-flow
```

The loop explicitly accumulates usage lists with `builtin.code` using `left + right` and stops after a reported model failure.
Its model configuration is passed into the loop through declared variables. The runner receives conversation
arguments and configuration, while the application owns database admission, process supervision, and saving.

## History metadata

History remains a list of user and assistant messages. Each message accepts an optional object-valued
`metadata` field; omitting it means no annotations. System instructions and the current prompt remain separate.

```json
{
  "role": "assistant",
  "content": "See the referenced document.",
  "metadata": {
    "citations": [{"uri": "https://example.com/reference"}],
    "agent": "researcher"
  }
}
```

Metadata is opaque application data. The LLM adapter forwards only role and content to the provider and
preserves existing metadata in its returned history. The chat application persists response metadata on the
new assistant message, including through save retries and database reopen. Returned history never replaces
stored messages or changes their metadata. New user messages currently have no metadata.

Metadata contributes to the canonical encoded history limit and the executable response limit. Existing
SQLite databases receive a forward migration; old messages retain absent metadata.

## Workflow results and reported calls

Executables declare `status`, `text`, and a nonoptional `usages` output. A successful result has nonempty
text; optional `metadata` belongs to that assistant message. `finish_reason` is optional. An optional `title` proposes a session title. Other workflow
outputs, including returned history, do not change persisted conversation state.

```json
{
  "status": "succeeded",
  "text": "Final answer",
  "metadata": {"agent": "researcher"},
  "usages": [
    {
      "call_id": "call-001",
      "node": {"id": "answer", "kind": "builtin.llm", "path": [{"node": "repeat", "index": 0}]},
      "model": "example-model",
      "attribution": {"purpose": "conversation", "profile": "default"},
      "usage": {"input_tokens": 120, "output_tokens": 35, "total_tokens": 155}
    }
  ]
}
```

Each usage entry describes one actual call attempt, including calls without reported token counters.
Its optional `attribution` object contains optional nonblank public `purpose` and `profile` labels.
Configure these labels on `builtin.llm` or assign them while composing usage lists. The LLM node records
them on successful and captured failed attempts. Accounting uses each supplied label independently,
falling back to the invocation's purpose or profile when absent. This preserves historical records and
workflows that omit attribution. Labels do not change provider behavior or node identity.
Each attempt also reports its start time in UTC Unix milliseconds; statistics use that time when available.
The node path identifies enclosing loop invocations; repeated executions of one node have distinct call IDs.
Node identity is available independently of observation. Direct node calls outside a workflow may lack node
information. Models are resolved by the calling node and must be public identifiers. The application does
not infer a model by inspecting the executable graph.

The workflow explicitly combines the lists it intends to report. Code list addition preserves list order and
values; it does not deduplicate or synthesize counters. Duplicate call IDs within a result are rejected.
Input, output, and total token counts are independent optional nonnegative integers; absent is unknown and
zero is a reported value. The backend never derives total tokens by adding other counters.

To migrate a workflow using the removed `builtin.concat`, declare the `mfn-code` package and replace the node with:

```json
{
  "id": "collect",
  "kind": "builtin.code",
  "config": {
    "language": "cel",
    "inputs": {
      "left": {"list": "object"},
      "right": {"list": "object"}
    },
    "code": {"value": "left + right"}
  }
}
```

The `left`, `right`, and `value` bindings remain unchanged. Use `{"list":"any"}` for arbitrary heterogeneous lists.
Recompile without `--locked` to refresh the workflow lock, then resume locked builds. The repository's example tests
generate their locks in temporary directories; no example locks are checked in.

Code applies its [documented limits](workflows.md): 1 MiB each for serialized inputs and outputs, 10,000 collection
entries across input and output conversion combined, and 16 nesting levels. Nested record fields count toward those
limits, so accumulated usages can fail before reaching the old concat node's accepted size. No partial list is
published on failure. Any secret input makes the entire Code output secret; business values remain available,
but public siblings are also redacted in observations. Internal shared payload identity is not preserved.

A failed result still reports all collected calls and a nonblank error summary, but requires no reply text:

```json
{
  "status": "failed",
  "error": "The final model request failed",
  "usages": [
    {
      "call_id": "call-002",
      "node": {"id": "answer", "kind": "builtin.llm"},
      "model": "example-model",
      "attribution": {"purpose": "conversation", "profile": "default"},
      "error": "Provider unavailable"
    }
  ]
}
```

Set `capture_errors: true` on LLM nodes that must return request failures as workflow data. Provider errors,
node deadlines, and response conversion failures then return failed status and a usage entry. The default
remains typed execution errors for existing workflows. Configuration/input failures and cancellation can
still abort execution before a final result is available. The workflow must wire status and usage outputs
through its chosen branches and accumulators. Successful status permits absent or empty error text.

## Persistence and recovery

Allocate an `Invocation`, durably begin its turn, and pass the resulting request to `contract.arguments`.
Supply `ModelIdentity { profile, model: None }` for workflow admission; actual model identities arrive
in `usages`. New calls do not compute or require configuration fingerprints. In-process callers pass the typed result to `reconcile_result`. Executable callers capture stdout within
`MAX_RECOVERY_BYTES` (16 MiB) and pass it to `reconcile_completion`. Recovery only needs the admitted
invocation and output bytes; it does not require a manifest or `ChatContract`.

Successful results become `CompletionState::Uncommitted`; `save_recovered` atomically saves the assistant
message, metadata, usages, optional title, and successful turn state. Failed results atomically save their usages and the
failed turn without creating an assistant message. Committed success wins over late cancellation or
conflicting output. If persistence fails, retain the recovery value or captured failure result and retry
storage only. No recovery operation invokes a model. Explicit abandon finalizes unfinished work.

One durable execution record can contain many reported model calls. Usage statistics expand these lists
inside SQLite and retain legacy call facts for existing records. `unreported_invocations` counts matching
executions without either a known single-model identity or a returned usage list. An explicit empty list
means no calls were reported; an interrupted execution with no result is not treated as confirmed zero.
A crash or cancellation before delivering the final result can lose otherwise completed call facts.

The current workflow output contract requires recompiling and updating older chat executables. The scalar
LLM `usage` output remains available to other workflows, but `mf_chat::ConversationResult` uses only `usages`. New
reply writes leave legacy scalar token columns empty. Existing successful single-model records with
reported scalar counters are upgraded to one usage entry using their stored call ID; explicit lists
and unknown counters are preserved. Legacy columns remain compatibility data. No terminal interface or implicit runtime usage collector is installed.

## Executable chat terminal

Compile a conversation workflow, then launch it with explicit local storage:

```bash
mf run ./chat-flow --tui --chat --chat-db ./chat.sqlite --chat-profile default --params.model your-model
```

The terminal presents one conversation above an input area that grows with its content. The initial empty chat is
saved only when its first prompt is sent; that prompt supplies its name. Existing conversations remain available
through the session picker. New chats use no terminal-supplied system instructions; resumed sessions retain their
stored instructions. Provider configuration remains in the workflow and its declared parameters.

| Interaction | Key |
| --- | --- |
| Send the prompt | Alt+Enter, or Esc then Enter |
| Insert a newline | Enter while editing |
| Browse messages / return to editing | Esc / i |
| Open commands | Ctrl+P |
| New chat / switch saved chat | Ctrl+N / Ctrl+S |
| Cancel the current execution | Ctrl+C |
| Quit after cleanup | Ctrl+Q |

Command and session pickers use Up/Down and Enter; Esc closes them. The command menu offers recovery actions only
when applicable. A retry starts a fresh execution of the latest unsuccessful turn. Saving a retained reply writes
the original result without running the workflow again; discarding it releases the execution lane explicitly.
Session switching and new chats wait until the current turn finishes, is cancelled, or its retained reply is resolved.
Additional submissions are rejected instead of queued. Drafts survive switching between idle conversations.

Session history opens at the latest bounded page. Earlier messages and Latest messages are available from the command
menu. Inspect current run opens the shared graph, trace, log, and data views; Esc returns to chat. Only the current
turn's observation is retained, and changing conversation or starting another turn releases it.

The terminal has no rename, archive/restore, usage dashboard, automatic-title, or parallel-execution controls.
Storage and programmatic usage queries continue through `mf-chat`. Every submission executes the complete workflow,
including loops, and persists its reported message metadata and individual call facts.

Unicode editing and bracketed paste are supported. Editors accept 256 KiB; workflow arguments and captured results
retain the shared 1 MiB and 16 MiB limits. Credentials travel through a private argument file. Storage operations and
process supervision run on one background worker, keeping terminal rendering and cancellation responsive.

Quitting cancels and reaps the child, reconciles retained results, and releases database ownership. It never prints
turn JSON to stdout. Handled turn failures allow a normal exit; display or cleanup failures return status 1. Unsaved
replies remain memory-only: restarting recovers abandoned turns as interrupted without resending them.
