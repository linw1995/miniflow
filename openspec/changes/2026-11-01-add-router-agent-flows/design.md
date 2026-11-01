## Caller-described Flow arguments

Workflow version 2026-11-01 adds Flow argument bindings containing a `source`, optional `description`,
and optional qualified `parameters` descriptions. Sources can reference files or built-ins, or declare
inline subgraph bodies with explicit outputs. One-node bodies use the same structure. Inline bodies
inherit the caller's workflow version and provider dependencies, like Loop/Iteration bodies.

Descriptions are static builder-supplied binding parameters, not fields on `WorkflowDefinition` or
changes to a built-in's parameter declarations. The same source can have different descriptions in
different bindings. Existing cluster annotations continue to describe node domains only. The LLM
adapter requires a nonblank binding description, merges parameter descriptions into the real input
schema, and rejects descriptions naming nonexistent inputs. Types, requiredness, defaults, and secret
handling remain derived from the real interface; model parameter names omit the sole startup node prefix.

## Generic compiler/runtime boundaries

Configurable built-in registrations receive JSON configuration and a map of resolved injected
`ResolvedFlowArgument`s retaining definitions and caller descriptions. The resolver resolves injection
paths in the caller directory, freezes source
bodies, merges dependencies, expands the provider definition, and resolves generated calls. Existing
zero-argument registrations remain supported.

Metadata-only node factories receive `SubgraphMetadata` (startup schema and output contracts), never
an executable closure. Runtime metadata has no Tool types, JSON Schema conversion, result encoding,
or LLM recovery policy.

## Routing and invocation

`mfn-core` provides `builtin.router` as an ordinary registered runtime task, directly usable in any
user-authored Flow without compiler rewriting or LLM tool expansion. It accepts a required string
`route` input, emits one exclusive boolean gate for a configured route (or `else` for an unknown
route), and skips the others. It performs no downstream execution; ordinary edges gate downstream
nodes, and consumers of unselected gates do not run. Provider expansion creates ordinary downstream
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
