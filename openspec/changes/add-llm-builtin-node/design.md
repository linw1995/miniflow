# Design

## Context

See [proposal.md](proposal.md) for the feature scope. `mf-runtime` currently has synchronous typed tasks and a bounded thread pool. A single execution domain can run inline on the calling thread. Execution contexts fork for parallel domains, stream messages, and subgraphs, while preserving shared runtime resources. Generated runners select node packages and their Cargo features through ordinary workflow dependencies.

The published `rig-core` and `rig-reqwest` 0.43.0 sources are available locally. The project pins Rust 1.98.1, above Rig's declared Rust 1.95.0 minimum. Rig's `Model::call` selects a unary reply. OpenAI's default completion route can use Responses; selecting `.chat(model)` explicitly selects Chat Completions.
`rig-core` separates portable contracts from optional HTTP transport. Its bundled native transport enters the current Tokio context or lazily initializes a process-wide one-worker fallback runtime. The caller retains ownership of the future; the adapter does not detach the operation.

Source references: [published core package](https://docs.rs/crate/rig-core/0.43.0), [published transport runtime](https://docs.rs/crate/rig-reqwest/0.43.0/source/src/runtime.rs), and [published OpenAI client](https://docs.rs/crate/rig-core/0.43.0/source/src/client/openai.rs). Implementation uses the published packages rather than APIs inferred from the moving upstream branch.

## Goals and non goals

Keep one successful invocation equivalent to one ordinary typed task result. Share future-driving support across providers without exposing a concrete runtime in node contracts. Preserve preparation without credentials, network access, or executor initialization. Keep each call's request, deadline, cancellation, and terminal response isolated.

Native asynchronous workflow scheduling, alternative production HTTP adapters, multiple vendor dialects, and generated typed fast paths for the LLM node are outside this change. A future executor is not an I/O reactor: using a portable executor with bundled reqwest still initializes Tokio in the transport.

## Decisions

### Package and execution contract

Add `mfn-llm` under `crates/builtin-nodes/llm`, using `NodeFactory::Plain`, `TypedTaskNode`, and `NodeValue` reflection. Export the node kind and the value contracts through the crate entry point following existing package conventions. Keep Rig-specific conversion and errors in the node crate. Do not add runtime or compiler branches for `builtin.llm`.

Each execution receives its conversation history explicitly and returns the updated transcript. The prepared node retains no conversation state. Reuse a lazily initialized, configured HTTP client within the prepared node; construct each request and model call independently. Do not cache credential-bearing configuration in the workflow definition, manifest, or snapshot metadata.
A crate-local bridge was considered, but optional shared future support avoids a separate executor policy in each future-based provider.

### Optional future execution

Add `mf-runtime` features `async` and `tokio`, both disabled by default. `async` enables a portable synchronous future driver. `tokio` also enables `async` and selects a managed native Tokio backend. Cargo features select the backend for the resolved runtime package; workflow JSON gains no executor selector. `mfn-llm` enables `mf-runtime/async`, while a `tokio` feature on `mfn-llm` forwards `mf-runtime/tokio` for users who want explicit runtime ownership.

Provide a generic future-driving method on `ExecutionContext`; its public signature uses standard Future and shared runtime error types. Keep backend machinery private. With `async` alone, drive the future using `futures::executor::block_on`. With `tokio`, lazily initialize one owned multi-thread runtime for the root execution scope and share it across forked contexts.
Keep that resource alive until all synchronous jobs using it have joined. Caller-owned contexts retain their resource until their scope ends. Direct node calls use the same lazy context resource. Node factories do not initialize it.

The bridge remains blocking: a pending HTTP call occupies its synchronous workflow thread or worker. Existing domain and iteration bounds continue to bound concurrent calls. Synchronous entry points called from an async host must run on that host's blocking thread facility; the bridge must reject incompatible nested executor entry with a typed error instead of panicking. Do not add `AsyncTaskNode`, spawn detached requests, or switch scheduling based on node kind.

A portable-only fixture must verify that `mf-runtime/async` and transport-free `rig-core` do not introduce Tokio. A fixture selecting `mfn-llm` with its bundled native transport will contain Tokio transitively. The managed backend provides explicit scoped ownership; the portable backend deliberately accepts Rig's process-wide fallback lifetime.

### Configuration and preparation

Use a strict deserialized configuration with the following fields:

| Field | Contract |
| --- | --- |
| `model` | Required nonblank model identifier |
| `base_url` | Default `https://api.openai.com/v1`; absolute HTTP or HTTPS URL without embedded credentials, query, or fragment |
| `api_key_env` | Default `OPENAI_API_KEY`; nonblank environment-variable name |
| `temperature` | Optional finite number in the inclusive range 0 to 2 |
| `max_tokens` | Optional positive output-token limit supported by the request representation |
| `timeout_ms` | Positive whole-call deadline, default 60000 milliseconds |
| `max_output_bytes` | Positive UTF-8 text limit, default 1048576 bytes |

Reject unknown fields, including `stream`, direct credential fields, and arbitrary provider request overrides. Validate numeric conversions and URL composition during preparation. The base URL is the API prefix; the selected Chat route appends `/chat/completions`. Honor only the configured credential-variable name and base URL, without implicit endpoint environment overrides.

Factories reflect contracts and validate static configuration only. They do not read credentials, create clients, start executors, or issue requests. Missing or empty credentials fail at execution with node context. Credential values must never enter workflow artifacts or diagnostic display. Retain typed provider error sources, while using safe node error display that does not print arbitrary remote response bodies or authentication headers.

### Inputs and terminal outputs

The fixed contracts participate in startup arguments and graph validation normally:

| Direction | Port | Runtime type | Required | Rust field |
| --- | --- | --- | --- | --- |
| Input | `prompt` | `string` | Yes | `String` |
| Input | `system` | `string` | No | `Option<String>` |
| Input | `history` | `list<object>` | No | `Option<Vec<ChatMessage>>` |
| Output | `text` | `string` | Yes | `String` |
| Output | `history` | `list<object>` | Yes | `Vec<ChatMessage>` |
| Output | `usage` | `map<int64>` | No | `Option<BTreeMap<String, i64>>` |
| Output | `finish_reason` | `string` | No | `Option<String>` |

`ChatMessage` is a nested `NodeValue` with required `role: String` and `content: String` fields. Its object codec rejects missing, unknown, and incorrectly typed fields; node-owned validation accepts only `user` and `assistant` roles and nonempty text.

The reflected collection descriptor is `List(Object)`, while nested codecs and business validation enforce the message shape and role values. Do not introduce enum codecs or a new runtime message type for this provider contract.

Missing `history` or an empty list starts a new conversation. Treat supplied history as already completed messages in chronological order; do not require strict role alternation or assume a turn count. The current `prompt` is a separate nonempty user message and must not already be appended by the caller. This ensures returned history can satisfy the next invocation's message contract. The node performs no implicit prompt templating.

Build each request in the order `system`, supplied history, current user prompt. Use Rig's `CompletionRequest::new(prompt).messages(converted_history)`, then add the optional preamble; `.messages` inserts history before the current prompt. Historical user and assistant entries map to their corresponding Rig message variants.

`system` is supplied separately on every invocation and is excluded from the history port. Reject `system`, `developer`, `tool`, and multimodal records in history instead of silently rewriting them.

On success, return `history = input_history + [user(prompt), assistant(text)]`. Preserve prior roles, content, and order exactly and append each new message once. Assemble history only after text and metadata conversion succeeds and publish it atomically with the reply.

Returning history is required even when no input history was supplied; failure, cancellation, or deadline expiry publishes neither the reply nor a changed history. The node does not mutate caller-owned input history or remember it for subsequent calls.

Workflows carry the returned history through a Loop variable, a subsequent LLM node, or external persistence. They also own trimming, summarization, and conversation selection; the node does not truncate, merge, deduplicate, or summarize transcripts automatically. Reusing a node with omitted history starts a fresh conversation. The configured output byte limit applies to the generated reply, not the supplied or returned transcript.

Use a unary `.chat(model).call(request)` operation. Concatenate normalized text blocks in response order and exclude reasoning from public text. Reject tool-call or unsupported output content, even when accompanied by text. Reject a response with no nonempty text. A nonempty response truncated by its token limit remains a successful terminal result carrying `length`; it is not described as an untruncated answer.

Expose `input_tokens`, `output_tokens`, and `total_tokens` when available in normalized usage. Do not invent zero values for absent counters; omit the usage port when none of these counts are available. Check conversion from unsigned provider counters to runtime signed integers. Map known finish reasons to stable lowercase names and preserve an unknown provider reason verbatim. Omit the finish-reason port when absent.

Convert and validate the entire response and updated transcript before returning one result. Errors publish no outputs. The text byte limit applies after the unary transport has buffered and decoded the response; it is an output limit, not a hard network-body memory bound. `max_tokens` provides an additional provider-side bound when honored.

### Deadlines and cancellation

Start the invocation deadline before credential and transport initialization; check elapsed time before polling the model call and race the pending call against the remaining deadline. Initialization is synchronous and is not forcibly preempted, but an expired deadline must prevent a subsequent request or result publication. On workflow cancellation or deadline expiry, drop the owned future and return a typed failure. No final result is published after cancellation.

Reuse the existing cancellation handle and a bounded waiter registration strategy; repeated polls must not accumulate unbounded wakers. The portable deadline mechanism must work without a current Tokio context. Preserve the model call's typed error source through Snafu context selectors. Disable HTTP redirect following and automatic request retries in the configured client; do not add node-level retries. Local future cancellation does not guarantee that a remote service stops work already accepted.

### Dependency policy and integration

Use `rig-core = =0.43.0` with default features disabled and explicitly enable `reqwest` and `rustls` in `mfn-llm`. Select a compatible configured `rig-reqwest` client for endpoint, redirect, and timeout policy. Keep dependency versions and default-feature policy in the workspace. No agent runtime, websocket transport, embeddings, or vector-store integration is needed.

The package follows ordinary dependency selection and inventory retention. Update release-support scripts, package fixture enumeration, plugin documentation, and license auditing alongside the new crate. Generated build and runtime preparation must reflect identical ports despite different credential environments. A workflow binary still needs network access and credentials at execution; it does not need Cargo or source files.

## Risks and tradeoffs

- Blocking bridge occupancy limits throughput: retain existing worker bounds and document this behavior before considering native async scheduling.
- Managed runtime shutdown and nested contexts can outlive a call: share one scoped owner and test forks, concurrent domains, direct calls, failure, and teardown.
- Rig fallback ownership is process-wide: document it explicitly and test the managed backend independently rather than assuming both have the same lifetime.
- OpenAI-compatible services have parameter differences: promise the selected Chat Completions contract, not every provider dialect, and test route and wire data with a local server.
- Unary responses are buffered before the output size check: document the boundary and reject excess text before publication.
- Explicit transcripts grow over successive turns: keep trimming and summarization under workflow control and surface provider context-limit failures without silently removing history.
- Upstream minor versions change APIs: pin the validated version and keep provider conversion isolated in `mfn-llm`.

## Migration plan

Group 3 introduces validated configuration and value contracts. Group 4 registers the complete executable factory together with its real provider operation, so intermediate commits do not expose a node backed by placeholder execution.

Add the optional runtime facility first, preserving default builds. Add the opt-in node package and terminal-only examples, then packaged runner and release acceptance. Existing flows need no edits. Adopting flows declare `mfn-llm` and supply credentials only when executed. Removing the package declaration and LLM nodes restores the original dependency graph. No persisted workflow data or schema migration is required.
