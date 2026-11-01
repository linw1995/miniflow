## ADDED Requirements

### Requirement: Describe named Flows as model tools

Workflow version 2026-11-01 SHALL accept an optional top-level annotation. Tool registration SHALL
require a nonblank annotation and SHALL publish it as the tool description. Tool names SHALL be Flow
aliases containing 1-64 ASCII letters, digits, underscores, or hyphens. Tool arguments SHALL be
derived from exactly one startup node. Secret inputs SHALL be hidden and require an environment or
default source. Tool Flows SHALL have terminal interfaces without stdin or stream outputs.

#### Scenario: Generate a calculator tool definition

- **WHEN** a calculator Flow annotates itself and exposes `operation`, `left`, and `right`
- **THEN** the `mfn-llm.agent` metadata adapter publishes one tool with that description and an object schema requiring those arguments

### Requirement: Dispatch tool calls concurrently and deterministically

`mfn-llm.tool-uses` SHALL compose routing, argument adaptation, ordinary Flow calls, and result
encoding. `workflow.router` SHALL only emit exclusive/skipped downstream gates. The runtime SHALL
expose unbiased metadata without Tool types or tool policy. Concurrent dispatch SHALL reuse parallel `builtin.iteration`, including its bounded worker queue, ordered result collection,
item isolation, and termination policy. Each invocation SHALL isolate Flow inputs, outputs, and loop
state while sharing cancellation. Unknown tools, invalid arguments,
missing inputs, and child failures SHALL become recoverable JSON error content. A blank call ID and
cancellation SHALL fail execution.

#### Scenario: Recover from one bad call

- **WHEN** a parallel Iteration dispatches one unknown tool and one valid call
- **THEN** the results retain input order, with an error object for the unknown tool and normal content for the valid call

### Requirement: Compose a multi-turn chat agent

The provider-owned `mfn-llm.agent-turn` Flow SHALL perform one model request with captured errors and
expose terminal response fields plus an optional streamed answer. `mfn-llm.agent` SHALL use a native
bounded Loop to append assistant and tool messages until completion, accumulate usages, and publish
only the final chat result.

#### Scenario: Answer after calculator calls

- **WHEN** the model requests multiple calculator calls and then returns final text
- **THEN** the second request contains the assistant call message and ordered tool results, and the agent returns the final text with both call usages
