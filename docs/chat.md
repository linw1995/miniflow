# Embedded chat core

The `mf-chat` library owns SQLite storage for multiple chat sessions. Its persisted operations are application-owned `mf_chat.*` nodes. It does not initialize a terminal or launch workflow runner processes.

## Database initialization

Open an explicit database path:

```rust
let database = mf_chat::ChatDatabase::open("state/chat.sqlite")?;
println!("{}", database.path().display());
```

The owner creates the parent directory, acquires an exclusive sibling lock, executes the initialization node, and keeps the lock until dropped. Another owning host receives a typed busy error. Startup applies embedded pending migrations and enables WAL, foreign keys, and a finite busy timeout. No external migration files or tools are needed.

Node preparation validates contracts without opening storage. Failures retain typed filesystem, connection, query, and migration sources through `ChatError`.

Persisted records distinguish sessions, ordered messages, conversation turns, and model calls. Title generation can therefore record model usage independently of chat messages. Schema rollback is an explicit development operation on disposable databases; startup only applies pending forward migrations.

## Sessions and history

Every host operation runs an application node through the existing workflow runtime:

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

`mf_chat.turn.complete` saves the assistant message, reported usage, and successful turn state together. Replaying the same call and response returns the saved completion. Reusing its identity with different text or metadata fails. A failure halfway through completion leaves no assistant message or partial usage update.

`mf_chat.turn.fail` and `mf_chat.call.fail` finalize unsuccessful work and cannot overwrite committed success. Error summaries are bounded, while live errors retain typed sources. `mf_chat.turn.load` and `mf_chat.call.load` expose individual records for inspection.

A model retry creates a fresh turn and call and retains `retry_of` pointing to the original unsuccessful turn. A persistence retry reuses the original identity and response and never invokes the model again. Their database transitions are distinct.

Opening a database under its ownership lock executes `mf_chat.storage.recover`: abandoned running work becomes interrupted and never automatically resends a request. A crash after a remote reply but before persistence can lose that reply and its usage. Interrupted usage remains unknown; it is not recorded as zero.

## Embedded conversation host

`ChatHost` owns the database, model profiles, and blocking execution workers. Each submission runs the embedded `conversation` module with a fresh context and cancellation handle. The `mf_chat.llm` node delegates to the existing LLM task using object envelopes, keeping absent optional fields inside produced values. No standalone workflow runner is spawned.

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

Profiles contain validated nonsecret LLM configuration. Credentials are read through the configured environment reference only during execution. Sessions keep their profile and system instructions; each call records the profile, model, and configuration fingerprint actually selected. An optional observer receives each configured flow's preparation and execution lifecycle.

Provider failures finalize begun work through failure nodes in fresh contexts. If cleanup also fails, `ChatError::Execution` retains both typed errors and invocation identities. A reply whose storage write fails returns `ChatError::Uncommitted`; pass its retained value to `retry_completion` or explicitly abandon it. Repeated persistence failures return the retained reply again and never resend a model request. `retry` starts a fresh model call for an unsuccessful original turn.

Run the two-turn example with an available model and credentials in the environment:

```sh
MF_CHAT_MODEL=your-model nix develop --command cargo run -p mf-chat --example conversation -- state/chat.sqlite
```

Set `MF_CHAT_BASE_URL` for an OpenAI-compatible endpoint. The example prints complete results with their session, turn, and call identities.

## Session title summaries

The first successful turn of a provisionally named session atomically creates one pending title call. The embedded `session_title` flow independently reads that completed turn, invokes the existing model adapter, and applies a concise title only while the captured title revision still matches. Generated titles are trimmed, nonempty, free of control characters, and at most 120 characters.

Automatic dispatch is enabled by default. `HostConfig::title_profile` selects another configured model profile; leaving it unset uses the conversation profile. Set `automatic_titles` to false to defer dispatch while keeping pending requests durable. Use `dispatch_titles`, `start_title`, or `retry_title` for explicit execution.

`poll_titles` collects completed background results and dispatch errors without blocking. Its result queue is bounded by the host concurrency limit; a full queue leaves new work pending in storage until results are collected. Foreground admissions take priority over title requests that have not started.

Manual names always take precedence. A rename before title execution skips the provider call; a rename during execution rejects the stale update but still saves reported usage. Rejected title text also retains usage. Title failures and cancellation preserve the successful conversation and do not add chat messages. Explicit retry creates a new title call; subsequent conversation turns do not automatically retry a failed title.

Reopening resumes never-started pending requests. Interrupted provider attempts are not resent. A title response whose write fails returns an uncommitted value with purpose `title`; use `retry_title_completion` to persist that retained response without another model request.

## Usage statistics

`usage` executes the embedded `usage_statistics` flow and its `mf_chat.usage.query` node. It reads committed call facts in one SQLite snapshot and never invokes a model or changes billing records.

```rust
let statistics = host.usage(mf_chat::UsageQuery {
    session_id: Some(session.id.clone()),
    purpose: Some(mf_chat::CallPurpose::Conversation),
    group_by: Some(mf_chat::UsageGrouping::Configuration),
    limit: 50,
    ..Default::default()
})?;
```

Filters accept session, profile, actual model name, configuration fingerprint, purpose, and call start times in UTC Unix milliseconds. Start is inclusive and end is exclusive. Group by session, model, configuration, profile, purpose, or UTC day. Pass `next_after` to fetch the next group page; the overall summary always covers the complete filter.

Each token counter exposes `known_subtotal`, `reported_calls`, and `missing_calls`. A missing observation is not zero; an explicit zero is a reported observation. No observations yield an unknown subtotal. Treat a subtotal with missing observations as partial coverage. Attempt and status counts include begun failed, cancelled, and interrupted calls, but exclude never-started title requests.

SQL computes aggregate facts without loading raw call history. Integer overflow fails explicitly. Calls from title generation remain attributed to `title`, including valid replies whose title update was stale or whose application title was rejected. Monetary estimates require separate pricing data and are not provided by the core.

## Executable conversation backend

`mf_chat::executable` provides the contracts and persistence lifecycle for an application that runs a
compiled chat workflow. The application owns process launch, cancellation, output capture, and scheduling;
the library owns conversation inputs, invocation identities, and durable state transitions.

The [chat workflow example](../examples/chat-flow.json) connects a `builtin.parameters` node to one
`builtin.llm` node. Compile it from the repository root:

```sh
nix develop --command cargo run -p mf-cli --features development-support -- compile examples/chat-flow.json --output target/chat-flow
```

Construct `ChatContract::from_manifest` from the executable's `WorkflowManifest` before opening storage.
Preflight accepts one finite LLM node, optionally preceded by one parameter node, with no stdin resource,
loops, or control edges. The parameter node must feed prompt, system, and history directly. Parameterized
flows must expose `text`, `history`, optional `usage`, and optional `finish_reason` from the LLM under those
exact names. Execute these flows as a whole; for a single LLM without a parameter node, use the runner's
existing `--node` option with `contract.node` to return all model ports independently of workflow aliases.

Resolve `contract.parameters` with `mf_runtime::resolve_workflow_inputs` before admitting work. This schema
excludes conversation inputs, so ordinary argument validation rejects caller-supplied prompt, system, or
history. Reuse the resolved configuration for each invocation. `model_identity` evaluates the model expression
against public parameters and fingerprints configuration without resolved secrets. Secret model identities
are rejected. The example reads credentials from `MODEL_API_KEY` and supports `params.model` and
`params.base_url` overrides without recompilation.

For each conversation:

1. Allocate an `Invocation` and call `begin_turn_with_invocation` to durably admit the turn and load successful
   history. Storage permits only one running conversation per session.
2. Pass the returned request and resolved configuration to `contract.arguments`. Absent system and history
   become an empty string and empty list for parameterized flows. Forward these arguments to the runner;
   the model process needs no database path or conversation identity.
3. Capture stdout within `MAX_RECOVERY_BYTES` (16 MiB), then call `reconcile_completion` with the invocation,
   captured reply, and unsuccessful exit classification. A valid reply from a running invocation becomes
   `CompletionState::Uncommitted`; already committed state always wins, including after late cancellation.
4. Persist an uncommitted reply with `save_recovered`. On failure, retain the value carried by
   `ChatError::Uncommitted` and retry saving or explicitly call `abandon_recovered`. Neither action invokes
   the model. If reconciliation itself fails, retain captured stdout until durable facts can be checked.

Malformed or missing output finalizes only the admitted invocation as failed, cancelled, or interrupted.
Diagnostics remain separate from the durable outcome. `retry_prompt` retrieves an unsuccessful turn and
its original prompt; a model retry must allocate fresh identities and pass the original turn as `retry_of`.

Recovery values live in application memory. A crash before persistence can lose the reply and its usage;
reopening storage interrupts abandoned work without resending it. Automatic title execution remains the
embedded host's responsibility; the executable backend does not dispatch pending title calls.
