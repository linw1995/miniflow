## Why

Reusable Flows already compose through `workflow.call`, but LLM tool calls still require callers to
hand-author schemas, route each tool name, and implement concurrent dispatch. This duplicates the
Flow contract and makes a multi-turn agent difficult to express and validate.

## What Changes

- Add static builder-supplied Flow argument descriptions and parameter descriptions, without modifying referenced definitions.
- Support inline Flow sources, including one-node subgraphs, alongside file and built-in references.
- Derive tool JSON Schema from the single startup node of each named tool Flow.
- Add tool-independent `builtin.router` as an ordinary registered runtime task in `mfn-core`, directly
  usable in user Flows without compiler rewriting; preserve exclusive/skipped gate mode and add
  explicit ownership and execution of passed prepared subgraphs.
- Add generic configurable built-in Flow expansion and metadata-only subgraph inspection.
- Defer metadata-dependent expansion until provider-linked preparation; bind linked tool interfaces and descriptions as ordinary Flow parameter data. The authored agent Flow projects the catalog once at runtime before its Loop, while argument adapters retain frozen interfaces without duplicate executable child definitions.
- Add `mfn-llm.tool-uses`, composing adapters, router-owned tool subgraphs, and native parallel Iteration.
- Add LLM-owned `agent-turn` and `agent` built-in Flows, reusing native Loop for model turns.
- Make the calculator example invoke the built-in agent with an injected calculator Flow.

## Impact

Extend workflow versioning, source resolution, compiler preparation and generated runners, runtime
metadata factories, and `mfn-core`. Add `mfn-llm` built-ins, examples, tests, and documentation. The LLM
node remains single-request and does not retain transcripts; agents own looping and history.
