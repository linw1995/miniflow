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
an executable closure. Metadata-dependent provider expansion is deferred until the selected node
providers are linked. Structural source resolution freezes requests and source dependencies without
requiring every node factory. Linked preparation derives interface facts and invokes an explicitly
pure provider expansion callback before final graph normalization and artifact generation.

The LLM provider uses that callback to bind linked interface definitions and builder descriptions
as the ordinary `params.tool_bindings` Flow parameter default, not to emit a final catalog constant.
The original authored `agent.json` contains a native Iteration mapping each binding through the
ordinary `llm.tool_definition` runtime node. Its recursive type projection shares the LLM adapter's
schema rules and never inspects raw executable JSON or resolves environment/default values.
The Iteration is outside the conversation Loop: it runs once per invocation and passes the collected
catalog into the Loop. Empty bindings follow the same path. The parameter has no environment binding;
no compile-time task evaluator is introduced. Executable definitions remain in the canonical passed
`flow_arguments` environment rather than being duplicated in catalog input data or call-local candidates.

The authored tool-uses graph has one shared arguments/executable-router/encode chain regardless of
tool count, with a legacy gate router guarding known/unknown input validation. Runtime argument
adapters consume the selected sanitized interface. The executable router explicitly receives its
owned subgraphs through `subgraphs: {"branch": {"parameter": "passed-name"}}`; preparation resolves
only these bindings from the canonical lexical environment. Branch names may alias parameter names.
The router selects and executes its prepared child itself, with no separate invocation node. A generic
reference-only `workflow.call` remains independently available. Runtime values remain ordinary JSON;
executable authority resides in `PreparedFlowArguments`, whose entries are `PreparedSubgraph` handles
constructed during preparation. All selectable targets are validated and generated before execution.
No source lookup, global binding fallback, opaque Value variant, or runtime definition interpreter is
introduced. Compile-time metadata and runtime configuration references do not add scheduling edges:
producers still require explicit prerequisites.

Loop/Iteration bodies retain lexical bindings; separately called children retain their own bindings.
Reference calls initially require terminal children without stdin, object arguments and captured
result envelopes. Observation paths identify callsite and binding, using distinct static body keys
without wrapper executable nodes. Generated runners and memory execution use the same prepared
binding contract. The generic runtime has no Tool types, JSON Schema conversion, tool-result encoding,
or LLM recovery policy.

## Routing and invocation

`mfn-core` provides `builtin.router` as an ordinary registered runtime task, directly usable in any
user-authored Flow without compiler rewriting or LLM tool expansion. It accepts a required string
`route` input, emits one exclusive boolean gate for a configured route (or `else` for an unknown
route), and skips the others. It performs no downstream execution; ordinary edges gate downstream
nodes, and consumers of unselected gates do not run. This legacy `routes` mode remains compatible
with plain factory instantiation. A separate `subgraphs` mode owns explicitly passed prepared children
and executes one using the shared call executor with object arguments and captured output/error
envelopes. Empty mappings are valid; mixed modes, invalid names and missing parameters fail
preparation. Host executors are looked up by parameter name and checked against every owned frozen
definition, including currently unselected branches. Branch aliases own their binding observation
paths. Interpreted preparation, kind-aware generated construction and manifests share the same selected
mapping. Cancellation and worker infrastructure failures remain failures.

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
