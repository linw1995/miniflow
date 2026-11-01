## Why

Reusable Flows already compose through `workflow.call`, but LLM tool calls still require callers to
hand-author schemas, route each tool name, and implement concurrent dispatch. This duplicates the
Flow contract and makes a multi-turn agent difficult to express and validate.

## What Changes

- Add optional Flow annotations and use them as model-visible tool descriptions.
- Derive tool JSON Schema from the single startup node of each named tool Flow.
- Add tool-independent `workflow.router` exclusive/skipped gates; downstream nodes perform all work.
- Add generic configurable built-in Flow expansion and metadata-only subgraph inspection.
- Add `mfn-llm.tool-uses`, composing router, adapters, ordinary calls, and native parallel Iteration.
- Add LLM-owned `agent-turn` and `agent` built-in Flows, reusing native Loop for model turns.
- Make the calculator example invoke the built-in agent with an injected calculator Flow.

## Impact

Extend workflow versioning, source resolution, compiler preparation and generated runners, runtime
metadata factories, and `mfn-core`. Add `mfn-llm` built-ins, examples, tests, and documentation. The LLM
node remains single-request and does not retain transcripts; agents own looping and history.
