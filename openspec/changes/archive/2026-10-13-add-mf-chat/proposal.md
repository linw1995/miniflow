# Persistent Chat with Embedded Workflows

## Why

[Issue #89](https://github.com/linw1995/miniflow/issues/89) calls for a chat application that demonstrates miniflow flows. A reusable `mf-chat` core needs durable, independent conversations and application-owned nodes before the existing terminal UI can manage chat sessions.

## What Changes

- Add `mf-chat` with SQLite persistence through `diesel` and `diesel_migrations`, embedded schema migrations, and short storage transactions.
- Register typed `mf_chat.*` nodes for storage initialization, session management, turn lifecycle, title generation support, and usage queries.
- Persist sessions, ordered messages, turns, and model calls separately; retain failed submissions while excluding them from model history.
- Embed conversation, session title, and usage statistics workflows as Rust modules with the existing compiler build helper and macro.
- Generate a session title after its first successful turn, preserve manual titles, and keep title failures independent of conversation completion.
- Save response data and reported usage in the same database transaction; aggregate conversation and title calls without treating missing token counts as zero.
- Expose host operations with explicit session and invocation identities, cancellation, per-session serialization, and bounded concurrency across sessions.

## Capabilities

### New Capabilities

- `chat-session-storage`: migrated SQLite storage, application nodes, multiple session management, ordered messages, and durable turn transitions.
- `chat-workflow-execution`: embedded conversation and title flows, runtime configuration, host lifecycle, cancellation, and failure recovery.
- `chat-usage-statistics`: durable model-call accounting and an embedded statistics flow with explicit handling of unknown usage.

### Modified Capabilities

None. Existing workflow embedding, LLM node, runtime, and terminal observation contracts are reused.

## Impact

Implementation affects a new `crates/mf-chat/` crate, workspace dependencies and lockfile, bundled migrations and workflow sources, Nix source inclusion, license auditing, documentation, and focused integration tests. Errors remain owned by `mf-chat` and retain typed sources.

This change plans the chat core. Integration into the existing `mf run --tui` experience remains the intended follow-up; terminal controls, command syntax, and its in-process or process transport are outside this change. It does not introduce a separate chat TUI, a persistent runner protocol, token streaming, automatic model retries, transcript summarization, or monetary cost estimates.
