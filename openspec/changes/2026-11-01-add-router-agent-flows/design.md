## Flow annotations

Workflow version 2026-11-01 adds an optional top-level `annotation`. The LLM adapter requires a
nonblank annotation for injected tools; ordinary calls and routers ignore it.

## Generic compiler/runtime boundaries

Configurable built-in registrations receive JSON configuration and a map of resolved injected
`WorkflowDefinition`s. The resolver resolves injection paths in the caller directory, freezes source
bodies, merges dependencies, expands the provider definition, and resolves generated calls. Existing
zero-argument registrations remain supported.

Metadata-only node factories receive `SubgraphMetadata` (startup schema and output contracts), never
an executable closure. Runtime metadata has no Tool types, JSON Schema conversion, result encoding,
or LLM recovery policy.

## Routing and invocation

`workflow.router` accepts a string route, emits one exclusive boolean gate (or `else`), and skips the
others. It performs no downstream execution. Provider expansion creates ordinary downstream
`workflow.call` nodes. Calls support generic object arguments and captured output/error envelopes;
cancellation and worker infrastructure failures remain failures.

The LLM provider owns canonical ToolDefinition/ToolCall/ToolResult values and the metadata adapters
that derive schemas, validate and map arguments, hide secret ports, and encode results. One startup
node per tool and a terminal interface without stdin/streams remain the supported tool contract.

## Agent composition

`mfn-llm.agent-turn` owns one model request. `mfn-llm.tool-uses` expands to a native parallel Iteration
whose body routes, adapts arguments, invokes the selected call, and encodes a tool result. Native
Iteration owns concurrency and ordered collection. `mfn-llm.agent` owns the bounded native Loop,
transcript, usages, and final status normalization. No node reimplements Iteration or Loop.

The calculator example only calls the built-in agent, injecting the calculator subgraph through its
Flow source declaration. The generated call body carries an observation-only provenance marker, so
compiled descriptions can project that injected subgraph; the marker does not affect execution.
