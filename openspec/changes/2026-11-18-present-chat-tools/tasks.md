## Implementation

- [x] Stream nonempty provider argument fragments before tool-call assembly.
- [x] Forward only the optional model-output stream through Agent and conversation adapters.
- [x] Preserve the dispatch graph and ordinary tool result encoding.
- [x] Render complete argument previews and authoritative results in the same independently foldable call group.
- [x] Highlight raw JSON arguments without altering the preview or persisted messages.
- [x] Index saved bodies and stream prefixes in SQLite and materialize body documents for the viewport.
- [x] Commit previews before display and remove replaced drafts atomically with terminal results.
- [x] Kill a streaming terminal and reopen its database to verify incomplete text and tool arguments without replay.
- [x] Derive and display invocation-local tool execution metrics from the existing tracing observations.
- [x] Validate fragmented, parallel, cancelled, saved, and source-free cases, repository hooks, and coverage.
