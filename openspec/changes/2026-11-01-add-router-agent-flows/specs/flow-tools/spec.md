## ADDED Requirements

### Requirement: Invoke passed frozen Flow parameters through a shared callsite

A reference-only `workflow.call` target SHALL select a string name exclusively from its lexical
passed frozen Flow argument environment. The same canonical environment SHALL supply prepared
callable bodies; callers SHALL NOT author a second call-local candidate table. Selectors SHALL NOT
load sources, fall back to global host bindings, or execute definition-shaped runtime JSON. Ordinary
Value/ValueRef JSON serialization SHALL remain unchanged. Static calls SHALL remain compatible.

Preparation SHALL validate all selectable targets. Initial reference calls SHALL require terminal
children without stdin, object arguments and captured result envelopes. Explicit producer
prerequisites SHALL remain necessary. Loop/Iteration bodies SHALL retain lexical bindings while
separate child calls SHALL NOT implicitly inherit them. Generated runners SHALL construct all bodies
from compiled plans and run after source removal. The authored tool-uses chain SHALL contain one
arguments/call/result adapter chain independent of tool count; runtime argument adaptation SHALL
consume selected sanitized interface data. Observation, snapshots and TUI bodies SHALL distinguish
bindings at the same callsite without inventing executable wrapper nodes.

#### Scenario: Shared authored dispatch invokes a passed binding

- **WHEN** a tool-use Iteration receives calls for two passed tools
- **THEN** the same authored arguments/call/result chain validates and invokes both bindings
- **AND** result order follows input order in memory and in the source-free generated runner

#### Scenario: A selector cannot forge executable authority

- **WHEN** a selector contains an unknown name, a global-only host binding name, or definition-shaped JSON
- **THEN** no unpassed body executes

#### Scenario: Binding-aware observation preserves identical child node names

- **WHEN** two passed bindings at one callsite both contain a node named `leaf`
- **THEN** their lifecycle and snapshot paths identify the selected binding separately
- **AND** the TUI can show both bodies under the single authored callsite

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

- **WHEN** the builder binds a calculator source with an overall description and descriptions for `calculate.operation`, `calculate.left`, and `calculate.right`
- **THEN** the `mfn-llm.agent` metadata adapter publishes one tool with the binding description and an object schema requiring `operation`, `left`, and `right`, including their descriptions
- **AND** the calculator definition remains unchanged

#### Scenario: Describe an inline single-node tool

- **WHEN** the builder binds a one-node inline body whose `calculate` node adds integer inputs `left` and `right` and exports `sum`
- **THEN** the tool schema exposes required integer properties `left` and `right` with the supplied descriptions
- **AND** model arguments are qualified as `calculate.left` and `calculate.right` before invocation, returning the JSON result `{"sum":5}` for inputs 2 and 3

### Requirement: Bind linked interfaces and explicitly project the catalog at runtime

The compiler SHALL retain metadata-dependent expansion requests through structural planning and
resolve them only where selected node providers are linked. Provider callbacks SHALL receive frozen
interface facts without executable child closures. Final generated artifacts SHALL contain expanded
graphs and frozen metadata, with no source-path reads or provider-interface reflection at startup.

The LLM provider SHALL bind names, descriptions, qualified parameter descriptions, and real linked
interface definitions as an ordinary Flow parameter default, without an environment binding.
The original authored agent Flow SHALL explicitly map these bindings through ordinary runtime
schema-projection nodes in a native Iteration before its conversation Loop. Projection SHALL run
once per agent invocation, including for an empty catalog, and the Loop SHALL reuse its result.
Expansion SHALL NOT replace a placeholder with a final catalog constant. Projection SHALL preserve
secret/default/environment semantics without resolving their values or guessing types from raw JSON.
Runtime argument validation SHALL retain only the required frozen interface, without embedding
another copy of the executable child definition. Dispatch MAY retain its existing provider expansion.
Metadata references SHALL NOT create execution dependencies or observation scopes.

#### Scenario: Compile a source-free agent with explicit catalog logic

- **WHEN** an agent is built with described injected tools
- **THEN** its final graph retains the authored catalog Iteration and projection node, supplied with linked interface data, alongside actual tool invocation calls
- **AND** arguments are validated against frozen interfaces and execution remains valid after source files are removed
- **AND** the source-free generated runner produces the same catalog and results as in-memory execution

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
