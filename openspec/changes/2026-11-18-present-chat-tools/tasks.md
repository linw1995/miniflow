## Implementation

- [x] Stream nonempty provider argument fragments before tool-call assembly.
- [x] Forward only the optional model-output stream through Agent and conversation adapters.
- [x] Preserve the dispatch graph and ordinary tool result encoding.
- [x] Render bounded argument previews and authoritative results in the same independently foldable call group.
- [x] Highlight JSON arguments and elide historical delta display without truncating persisted messages.
- [x] Derive and display invocation-local tool execution metrics from the existing tracing observations.
- [x] Validate fragmented, parallel, cancelled, saved, and source-free cases, repository hooks, and coverage.
