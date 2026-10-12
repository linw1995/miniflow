# Design

## Context

See [proposal.md](proposal.md) for the scope and issue reference. The existing embedding facility emits structural workflow modules and validates linked nodes during host preparation. `builtin.llm` accepts explicit prompt, system, and history inputs and publishes complete text with optional usage. It retains no conversation state.

`mf-tui` currently owns one externally launched runner, its terminal, output spool, and OTLP receiver. This change delivers the reusable application core first. The existing TUI remains the intended integration destination; selecting a transport or command syntax belongs to that subsequent change.

Use Diesel SQLite, embedded migrations, bundled SQLite, and per-connection pragmas. Separate model requests from short database transactions and retain this repository's Snafu errors and node preparation contracts.

## Goals / Non-Goals

Goals are durable multi-session operation, real workflow composition through application nodes, explicit invocation identities, and recoverable storage transitions. SQLite is authoritative for saved messages and usage; the host owns execution and scheduling, and a future UI owns selection and drafts.

Terminal widgets, generic runner changes, native async workflow scheduling, provider retries, conversation branches, automatic context trimming or summarization, and monetary pricing are outside the core. Session title summarization does not summarize or replace model history.

## Decisions

### 1. Keep the application boundary in one crate

Add `crates/mf-chat/` as a library with public host operations, value contracts, embedded flow entry points, and registered `mf_chat.*` nodes. Keep Diesel schema, row models, connection machinery, and repository helpers private unless an actual module boundary requires sharing. Choose exports at the crate entry point rather than mechanically applying scoped visibility or creating extra facade crates.

The crate depends normally on `mf-runtime`, `mf-compiler` with default features disabled, `mfn-llm`, registration and serialization utilities, and its storage dependencies. Its build dependency enables only compiler `codegen`. The compiler and runtime never depend on `mf-chat`. No terminal dependency is needed for the core.

An application-specific node vocabulary makes persisted operations visible in workflow composition and observation. A generic SQL node was rejected because arbitrary SQL would hide session invariants, weaken typed contracts, and expose database details to callers.

### 2. Embed all three workflows in the crate

Keep definitions under `workflows/` and submit them together to `embed_workflows` from `build.rs`. Include them with `embed_workflow!` as `conversation`, `session_title`, and `usage_statistics` modules. Link application and LLM registrations through the ordinary host registry; never link node dependencies into the build script or create nested Cargo builds.

The host loads a module's structural plan, applies selected nonsecret configuration only to its designated LLM node, and calls existing compilation/preparation APIs. Derive observation identity from that configured plan. Do not add a generic config override API to the compiler solely for this application. Conversation and title models can use separate configuration profiles.

Each session retains its conversation profile reference and system instructions; the host configuration resolves the conversation and title profiles. Missing profiles fail before beginning work. Each model call stores the resolved model name, profile reference, and a fingerprint of the validated nonsecret configuration. Credentials remain environment references and are read only by the LLM node.

Initially prepare flows as needed instead of adding a hidden prepared-flow cache. Every invocation receives a fresh execution context and cancellation handle. Repeated preparation does not mutate durable session state. Workflow definitions and migrations are included in Cargo packages and Nix sources, so source-free execution works after building.

### 3. Initialize storage once and connect only during node execution

Use workspace-managed `diesel` and `diesel_migrations` dependencies in the 2.3 family and the compatible `libsqlite3-sys` bundled feature. Resolve exact versions in the workspace lockfile during implementation and verify that native SQLite linkage is unique. Use file-backed temporary databases in integration tests.

The host takes an explicit database path. Acquire an exclusive advisory ownership lock on a stable sibling lock file before initialization. A second owning host returns a typed busy error. This initial single-owner boundary supports many concurrent sessions within one host and makes restart recovery safe without adding process leases. Readers that do not own application execution do not perform recovery.

Execute `mf_chat.storage.initialize` after acquiring ownership. It creates the required parent directory, opens the database, configures WAL, foreign keys, and a finite busy timeout, and runs embedded pending migrations. Release ownership if initialization fails.

Every storage node receives the database locator explicitly as a typed value; it does not obtain a database through global environment mutation or a process-global singleton. Use short-lived connections per storage operation, applying connection-local foreign key and timeout settings. Do not rerun migrations in every node. Add a connection pool only after measuring a need.

Factories validate static configuration and reflect ports without touching storage. This respects existing metadata preparation and avoids requiring captured resources in the registry's function-pointer factories.

Use `embed_migrations!("migrations")` and explicitly track the migration directory in the existing build script. The macro requires that additional rebuild tracking when migration files change; see the [Diesel documentation](https://docs.rs/diesel_migrations/latest/diesel_migrations/macro.embed_migrations.html). Maintain `diesel.toml` and the checked-in generated schema beside the migrations.

### 4. Store four distinct domain records

| Table | Main data and invariants |
| --- | --- |
| `sessions` | Stable ID, provisional/generated/manual title, title revision, conversation revision, profile reference, system instructions, archive state, timestamps |
| `turns` | Stable ID, session ID, optional `retry_of`, session order, state, timestamps, safe failure summary |
| `messages` | Stable ID, session and turn IDs, per-session sequence, role, content; one user and at most one assistant message per turn |
| `model_calls` | Stable ID, session ID, optional turn ID, purpose, profile/model/configuration identity, title source revision, state, timestamps, nullable token counters, finish reason and safe failure context |

Use explicit domain identifiers in Rust and stable string encodings at database and node value boundaries. Add foreign keys, role/state/purpose checks, unique message sequence and turn-role constraints, and an index enforcing at most one running conversation turn per session. Index session ordering and call filters. Message order must not depend on timestamps or UUID sort order.

Turn states are `running`, `succeeded`, `failed`, `cancelled`, and `interrupted`. Call states additionally include `pending` for scheduled title work and `skipped` for obsolete work that never invokes a provider. A title call can reference the first successful source turn without owning or changing that turn's state. Pending rows record a profile hint; actual model identity is captured at invocation.

Archiving preserves all records and prevents new conversation turns; restoring permits new turns again. A request already begun may finish into an archived session. Manual rename increments the title revision and marks its source manual. Session and message listings use bounded keyset pages; complete model history remains a separate operation.

An entire history JSON blob was rejected as the primary representation because ordered messages and independent call facts support pagination, failed-turn inspection, and aggregates without rewriting a whole transcript. First-version history contains all successful messages up to a configured finite UTF-8 byte bound, defaulting to 1 MiB of canonical history JSON. Exceeding the bound fails before model execution; it does not silently trim saved data.

### 5. Make a turn a pair of short storage transactions around one model call

The conversation definition follows this shape:

```text
mf_chat.turn.begin -> mf_chat.llm -> mf_chat.turn.complete
```

The host prepares the configured flow, allocates session-bound turn and call IDs, and supplies them with the database locator and prompt to the initial node. `turn.begin` validates the session, loads successful history in order, and checks its bound. In one short transaction it persists the user message, running turn, and running call, then releases the connection before the provider executes. It forwards the locator, identities, prompt, system, and history through ordinary data edges.

The LLM receives completed history without the current user message. `turn.complete` uses the returned text and optional metadata to insert one assistant message, finalize call facts and turn state, advance the conversation revision, and update session activity atomically. It never reinserts the full returned LLM history. Typed decoding and domain validation finish before the write transaction.

Use a thin `mf_chat.llm` adapter around the existing prepared LLM task. Its required object request and response ports contain optional system instructions and metadata inside their value codecs. Ordinary edges cannot consume an omitted output port, so connecting optional `builtin.llm` ports directly would fail or skip persistence. The adapter adds no provider logic, retries, or runtime special cases. SQLite messages remain authoritative; the adapter does not forward the redundant returned history.

Use short immediate write transactions where read/check/write operations require a stable decision. Retry only bounded database contention around idempotent storage work. Do not retry HTTP requests, and do not hold a transaction or connection lock over model I/O. SQLite permits only one concurrent writer; see its [transaction contract](https://www.sqlite.org/lang_transaction.html).

The completion transaction also schedules the first automatic title call when the session is still provisionally titled. The pending call row is the durable work item, avoiding a fifth queue table and the gap between successful conversation commit and title scheduling.

### 6. Separate commit retries, model retries, and failure cleanup

Completion is idempotent by call ID and validates that the replayed text and metadata match the stored result. Equivalent replay returns the saved completion; conflicting replay fails. A model retry allocates a new turn and call with `retry_of` pointing at the original unsuccessful turn. Its history reflects the successful conversation at the time the new turn begins.

Existing flows stop when the LLM node fails; a downstream storage node cannot serve as a finally handler. The host therefore invokes `mf_chat.turn.fail` or `mf_chat.call.fail` through prepared isolated task targets or small embedded maintenance flows. Session management operations use the same approach. Do not add error edges or runtime special cases for chat.

Cleanup uses a fresh context with a fresh cancellation handle. It conditionally finalizes running work and cannot overwrite a committed success. If cleanup itself fails, preserve both typed errors and the unresolved IDs. Startup recovery under exclusive ownership marks abandoned running turns and calls interrupted. Pending title calls remain pending; interrupted calls are never automatically resent.

If the LLM succeeds but completion fails, retrieve its validated intermediate outputs from the caller-owned execution context and return an uncommitted completion value carrying the response and identities. The host retains the session's running reservation while that value is pending.

The caller can retry persistence using the completion task alone or abandon it through finalization. This path never repeats the provider request. Shutdown finalizes any remaining uncommitted work before releasing ownership.

Database idempotency is not exactly-once provider execution: a crash between receiving a remote reply and saving it can lose its text and usage. Recover such work as interrupted with unknown usage, without claiming that the provider consumed zero tokens. A durable remote reply journal or provider-specific idempotency is a later feature.

### 7. Generate titles independently with revision checks

The title definition follows this shape:

```text
mf_chat.title.prepare -> mf_chat.llm -> mf_chat.title.complete
```

After the first successful turn returns, the host schedules its pending title work within the total concurrency limit. `title.prepare` claims the pending call, checks that the title remains provisional, captures its revision and the first completed turn, and builds a separate summarization prompt. A manual rename before execution skips the pending job without invoking the model.

Keep completed background results in a queue bounded by the host concurrency limit. When that queue is full, additional title requests remain pending in SQLite until the caller collects results. Callers can defer automatic dispatch while retaining durable scheduling, start a title job explicitly, or select a dedicated title profile. Foreground admission reserves priority before session reads so not-yet-started background work yields.

`title.complete` validates and normalizes a nonempty single-line title, then saves call facts and conditionally applies it in one transaction. A title changed after preparation retains precedence; the response is accounted for even if its title update is skipped. A rejected application title still retains any usable LLM usage and a title-specific outcome. Title calls never append chat messages or alter the source turn's success.

Failure or cancellation leaves the provisional title and committed conversation intact. Subsequent conversation turns do not schedule another automatic attempt. Provide an explicit retry operation with a fresh call identity for failed or interrupted title work while the title remains provisional. Resume never-started pending work after reopening.

An automatic title on every turn was rejected because it adds model traffic and races with manual naming without a clear first-version benefit.

### 8. Derive statistics from immutable completed call facts

The statistics module can be a one-node flow using `mf_chat.usage.query`; SQL query and aggregate logic belong to that typed domain node. Additional pass-through nodes solely to demonstrate a larger graph are unnecessary. Store response usage during completion, rather than running a second write-oriented statistics flow after the response commits.

Query begun calls in one read snapshot. Support a session or all sessions, optional profile/model configuration and purpose filters, inclusive UTC start and exclusive UTC end, and optional grouping by session, configuration, purpose, or UTC day. Compute the overall aggregate over the complete filter and page grouped rows with a bounded limit. Do not load all raw call rows into memory.

For each token counter return a nullable known subtotal plus reported and missing counts. Explicit zero is reported data; no observations produce an unknown subtotal. Partial coverage remains labelled partial. Track attempted-call counts by state; unstarted pending title work is excluded. SQL or Rust arithmetic must fail on integer overflow. Keep UTC timestamps in a queryable, consistent encoding.

Call purpose determines attribution. A valid title reply discarded after a rename still counts under `title`. Failed, cancelled, and interrupted calls count as attempts but contribute no invented token values. No provider request or statistics fact mutation occurs during aggregation. Cost estimates require a later pricing model.

### 9. Keep host scheduling bounded and independent of UI selection

Expose a host service with open/close, session operations, submit/retry, title retry, and usage operations. Results and events always carry stable identities. Implement synchronous storage and flow execution on host-owned blocking workers; the future TUI must not execute those entry points in its render/input loop.

Admit only one running conversation per session and default to four active workflows across the host. Additional foreground submissions receive an explicit capacity result; pending title work waits durably until a slot is available. Foreground work has priority over not-yet-started title jobs. Runtime domain limits remain separately enforced; a prepared flow's internal worker bound is not a host-wide invocation bound.

Changing a UI selection does not cancel or redirect an invocation. On shutdown, reject new work, request cancellation, join all owned jobs, attempt finalization, and only then release database ownership. Database calls retain finite waits; cancellation remains cooperative across synchronous setup and storage operations.

## Risks / Trade-offs

- A single owning host excludes simultaneous chat writers in separate processes. This keeps recovery reliable without process leases; multi-session concurrency remains available inside the host.
- A provider may consume tokens despite cancellation or a crash before persistence. Unknown usage is preserved explicitly; no automatic resend or fabricated total hides that boundary.
- Reply persistence can fail after a paid provider call. Retaining validated intermediate output permits database-only retry while the owning host remains alive.
- A long transcript can exceed the history bound or provider context limit. Fail explicitly and preserve stored messages; automatic trimming and summarization are deferred.
- SQLite native linkage and embedded SQL/workflow source filtering can fail packaging independently of local tests. Validate Cargo package contents, license notices, and the Nix build.
- Title and conversation completion can race with renaming or cancellation. Stable IDs, conditional state transitions, revision checks, and transactional facts prevent stale overwrites.

## Migration Plan

Introduce the crate and initial schema without touching existing user data. Add nodes together with their actual implementation and tests; do not expose placeholder model or storage behavior. Add the embedded flows and a source-free host example, then validate package inputs and integration.

Existing workflows and TUI commands require no changes for this core. Keep SQL migrations in versioned directories with matching up/down files and verify fresh initialization and reopen. Production startup applies forward pending migrations only; reverting schema is an explicit development operation, not an automatic response to initialization failure. Removing the core leaves its database preserved for a compatible later version.

After the core is reviewed, a separate TUI integration change will choose how the existing `mf run --tui` experience hosts and observes these application flows. It must preserve the explicit session and call identities rather than returning to process-per-turn conversation state.
