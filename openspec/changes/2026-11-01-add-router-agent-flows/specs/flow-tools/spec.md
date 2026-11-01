## ADDED Requirements

### Requirement: Describe named Flows as model tools

Workflow version 2026-11-01 SHALL accept injected Flow arguments containing a source and optional
builder-supplied overall and qualified parameter descriptions. A source SHALL support a file path,
a built-in reference, or an inline subgraph body with explicit outputs. Inline bodies SHALL inherit
the enclosing version and provider dependencies; a single node SHALL use a one-node body.
Descriptions SHALL be static binding metadata and SHALL NOT modify the referenced definition or
its parameter declarations. Cluster annotations SHALL retain their display-only node-domain meaning.

Tool registration SHALL require a nonblank binding description and SHALL publish it as the tool
description. Tool names SHALL be Flow aliases containing 1-64 ASCII letters, digits, underscores,
or hyphens. Tool arguments SHALL be derived from exactly one startup node. Builder parameter
descriptions SHALL be merged into the matching schema properties; nonexistent input references
SHALL fail preparation. Types and requiredness SHALL remain derived from the real interface.
Secret inputs SHALL be hidden and require an environment or default source. Tool Flows SHALL have
terminal interfaces without stdin or stream outputs.

#### Scenario: Generate a calculator tool definition

- **WHEN** the builder binds a calculator source with an overall description and descriptions for `params.operation`, `params.left`, and `params.right`
- **THEN** the `mfn-llm.agent` metadata adapter publishes one tool with the binding description and an object schema requiring `operation`, `left`, and `right`, including their descriptions
- **AND** the calculator definition remains unchanged

#### Scenario: Describe an inline single-node tool

- **WHEN** the builder binds a one-node inline body whose `calculate` node adds integer inputs `left` and `right` and exports `sum`
- **THEN** the tool schema exposes required integer properties `left` and `right` with the supplied descriptions
- **AND** model arguments are qualified as `calculate.left` and `calculate.right` before invocation, returning the JSON result `{"sum":5}` for inputs 2 and 3

### Requirement: Route user Flows with an ordinary runtime task

`mfn-core` SHALL register `builtin.router` as an ordinary runtime task directly usable in user-authored
Flows without compiler rewriting or LLM tool expansion. It SHALL accept nonempty configured `routes`
and a required string `route` input, emit exactly one exclusive boolean gate for the matching route or
`else` for an unknown route, and skip all unselected outputs. Downstream execution SHALL use ordinary
edges rather than router-owned invocation.

#### Scenario: Route a user Flow without tool expansion

- **GIVEN** a user Flow declares `mfn-core` and a `builtin.router` with routes `first` and `second`, with downstream tasks gated by each route and `else`
- **WHEN** its `route` input is `second`
- **THEN** only the `second` gate is emitted and its downstream task runs; tasks gated by `first` or `else` do not run
- **WHEN** its `route` input is unknown
- **THEN** only the `else` gate is emitted and its downstream task runs; tasks gated by named routes do not run

### Requirement: Dispatch tool calls concurrently and deterministically

`mfn-llm.tool-uses` SHALL compose routing, argument adaptation, ordinary Flow calls, and result
encoding. `builtin.router` SHALL only emit exclusive/skipped downstream gates. The runtime SHALL
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
