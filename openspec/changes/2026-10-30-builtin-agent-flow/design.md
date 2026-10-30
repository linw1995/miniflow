# Design

The LLM accepts explicit tool definitions and produces protocol calls without reading tool topology.
A metadata catalog executes before each model request and inspects the actual prepared tool workflow.
It discovers its public Dispatch selector/arguments entry, including through reusable Calls, then
reflects decoded receiver schemas, sensitivity, and explicit field connections. No executor, default
value, or environment name is exposed by graph metadata.

`builtin.dispatch` is a core dataflow node. A selected branch publishes one gate and its declared
parameter values. Other branch outputs are skipped. Gates use ordinary execution edges; values use
ordinary input bindings. Explicitly routed fields are required on selection. Other upstreams and
node defaults remain separate bindings. Tool execution and output processing belong to the existing
scheduler, with the existing typed failures, cancellation, and limits.

The built-in agent uses a normal Loop for model rounds. A protocol adapter merges shared tool inputs
with per-call selector/arguments and creates WorkflowArguments for one argument-mode Iteration. Each
item calls the supplied tool workflow. A single result list is correlated with original call IDs in
model order, including repeated and mixed tool names. Results are redacted before bounded protocol
encoding. The agent exposes optional text deltas using ordinary Loop and Call child-stream exports.
Unobserved exports preserve unary execution.

`workflow.call.config.flows` binds a reusable Flow's named sources. Relative binding paths belong to
the caller, not the child. The existing resolver expands and freezes every body before runtime; no
new executor, invocation table, or runtime Flow lookup is introduced. Caller-selected provider sources
retain authority over built-in dependencies.

Generic argument-mode Iteration, structural input schemas, and prepared graph metadata remain shared
foundation APIs. The LLM-specific grouped batch outputs and Iteration-only tool-discovery restriction
are removed. The example imports a normal tool router containing Dispatch, Multiply Flow, formatting,
and another ordinary tool, with one exclusive result merge.
