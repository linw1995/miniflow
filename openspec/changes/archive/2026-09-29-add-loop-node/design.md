# Design

## Context

`WorkflowDefinition` currently contains one flat DAG. `CompiledWorkflow::generate_artifacts` emits one Rust call per node. `Flow::execute_in_context` traverses one topological order, and `ExecutionContext` stores a single output value or skip marker per qualified output ID. The observation protocol and TUI assume one lifecycle per node per run. Repeating a body in that representation would overwrite prior outputs and violate event sequence, state, and description invariants.

The [Dify Loop documentation](https://docs.dify.ai/en/cloud/use-dify/nodes/loop) is the behavioral reference. The JSON format below is a miniflow format, not a Dify export schema. Dify's separate [Iteration node](https://docs.dify.ai/en/cloud/use-dify/nodes/iteration) processes array items and may run in parallel; that behavior is not part of this change.

## Decisions

### 1. Use an explicit container with a built-in declaration

Add definition version `2026-09-29` while continuing to accept `2026-09-26` definitions unchanged.
A node with kind `workflow.loop` has a typed `loop` field rather than an opaque `config` payload.
`mfn-core` registers the Loop declaration; the compiler requires that registration and replaces the
declaration with an engine-prepared executor. The engine reserves `workflow.loop_assign` and
`workflow.exit_loop` for use inside a loop body. These control kinds cannot be supplied by plugin
registrations. All ordinary body nodes still resolve through the Flow's declared `dependencies`.

The loop field declares `max_iterations`, a nonempty list of `{name, type}` variables, an optional
`until` condition, and a `body` with `nodes`, `edges`, and `control_edges`. `max_iterations` is
required and must be in `1..=1000`. A variable has a required input port on the Loop container for
its initial value and a required output port of the same type for its final value. Move type
descriptor parsing into shared support: Loop variables may use every existing `ValueType` category,
including broad `any`, `array`, and `object`, while `builtin.code` continues to accept only its
current concrete subset. Both use the same nested `list` and `map` syntax and depth bound. Reserve
`index` as a body-only, zero-based integer supplied by the engine.

Within the body, `$loop` is a synthetic source with one output per variable plus `index`. It has no
plugin registration or runnable step. Edges from `$loop` bind values to ordinary inputs; control
edges can also use its outputs. The body uses local node IDs, and `$loop` is reserved there. A
nested Loop receives initial values through explicit edges in its containing body. Outer output IDs
are not implicitly visible inside a body. Plugin context references resolve only against producers
in the current body and `$loop`, with the existing explicit-ancestor rule. Qualified output ID
collisions are checked within each scope.

For example, this node receives its initial `count` from an outer edge and exposes its final `count` to downstream nodes:

```json
{
  "id": "repeat",
  "kind": "workflow.loop",
  "loop": {
    "max_iterations": 10,
    "variables": [{"name": "count", "type": "int"}],
    "until": {"variable": "count", "operator": "gte", "value": 3},
    "body": {
      "nodes": [
        {
          "id": "increment",
          "kind": "builtin.code",
          "config": {
            "language": "cel",
            "inputs": {"count": "int"},
            "code": {"next": "count + 1"}
          }
        },
        {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "count"}}
      ],
      "edges": [
        {"from_node": "$loop", "from_output": "count", "to_node": "increment", "to_input": "count"},
        {"from_node": "increment", "from_output": "next", "to_node": "assign", "to_input": "value"}
      ]
    }
  }
}
```

`workflow.loop_assign` has one required `value` input typed as its target variable and one `done`
control output. Its step commits the state write and `done` publication together after input
validation. A skipped assignment leaves the variable unchanged. A later step that must observe the
write must have an explicit data or control dependency on the assignment; independent ordering by
node ID is not an ordering contract. `workflow.exit_loop` has no data ports, is activated by control
dependencies, and exits the nearest enclosing Loop immediately when reached. A skipped exit does
nothing. Both kinds are invalid at the top level.

The `until` condition reads a declared loop variable after a complete body pass. It uses the existing scalar comparison operators and numeric comparison semantics from `builtin.if_else`; extract those semantics into shared support. It cannot reference an arbitrary body output that may be absent on a branch. An absent `until` means the loop runs to `max_iterations` unless an exit step runs. Loop variables remain local to one workflow run; this change does not add conversation persistence.

### 2. Keep each body a DAG and give every pass a fresh frame

Plan the outer graph and each body recursively. Every scope has a deterministic topological order,
and every data/control edge stays within that scope except the synthetic `$loop` source. Reject
cycles, missing endpoints, cross-scope edges, duplicate bindings, unknown update targets, invalid
conditions, and missing required inputs before installing a binary. Validate unreachable body
branches and plugin configurations as rigorously as top-level nodes. Limit nesting to four Loop
containers and use a single per-run budget of 10,000 scheduled steps across all scopes; skipped
steps count because they still resolve dependencies.

At runtime, resolve the container's initial inputs exactly once. If any incoming dependency is
skipped, skip the Loop and all of its declared outputs without entering the body. Otherwise, create
a typed variable map. Each pass creates a fresh body output frame, seeds `$loop` from the current
variable map and the zero-based `index`, and runs body steps in order. Only assignments mutate the
variable map; body node outputs and skip markers are discarded when that pass ends. The next pass
sees the updated variables. No body output is published into the parent scope except final Loop
variable outputs. This also prevents an old body output from satisfying a dependency in a later
pass.

After a full pass, test `until`; a true result ends the Loop. Reaching `max_iterations` also ends
successfully and publishes current variables. If both happen on the same pass, the condition is the
recorded stop reason. An activated `workflow.exit_loop` stops the remaining body steps immediately,
preserves writes already completed in that pass, and publishes current variables. There is always at
least one pass for an active Loop. A body failure or budget exhaustion aborts the workflow and
publishes no Loop outputs; already completed plugin side effects are not rolled back. Errors
identify the nested Loop path, body node, and phase, plus the pass index for failures during
execution. Preparation failures occur before any pass and have no pass index. The shared executor
validates every input, output, and assignment before publication.

Use an explicit frame stack in `ExecutionContext`, not string-concatenated global keys. A frame owns
only its local completed outputs and has a reference to its enclosing Loop variable map. Ordinary
`Node::execute_with_context` remains read-only; engine-owned assignment and exit steps use dedicated
runtime operations. `Flow` and the generated runner both call these operations. Generated Rust
prepares ordinary plugin instances once, then emits a structured loop around the body's ordered
calls. Type inference treats `$loop` values as declared types with unknown runtime values, so an
initial constant cannot incorrectly specialize later passes.

### 3. Observe actual invocations and preserve uncertainty

Version `--describe` and lifecycle events together. The description contains the outer graph and
nested body graphs without configuration, variable values, predicates, or business data. An event
identifies a node invocation with its local node ID and a structured path of enclosing Loop IDs and
zero-based pass indices. The path is a list rather than a delimited string, since definition IDs are
opaque. Top-level nodes have an empty path. An invocation has one start and one terminal outcome;
repeated Loop passes are separate invocations, not retries.

Emit `mf.loop.pass.started` before each body traversal and `mf.loop.pass.finished` after each
completed, exited, or failed pass. A pass finish carries its local visited prefix, so a reached exit
proves which later body steps were not run. Keep the per-run sequence and lightweight workflow final
boundary. Replace the current static `2 * node_count + 2` event bound with a checked bound of `4 *
10,000 + 4`: at most two node and two pass records per scheduled step, plus one failed pass that
cannot schedule its first step after budget exhaustion. Record actual visited-step
counts and the currently active frame/prefix on failure; a future pass that never started is not a
skipped node. Loop completion records include pass count and a stop reason (`condition`, `maximum`,
or `exit`) but no variable values. A skipped Loop emits a skip outcome without creating pass
records. Old event and description versions retain their existing validation rules for old binaries.

The TUI groups a Loop as one outer graph node, shows its active pass and stop reason, and lets the
user inspect the current body graph and up to 64 recent pass frames across the run. Aggregate pass
counts stay visible when older details are evicted. Deduplication and state transitions use
`(run_id, invocation path, node ID)`, while sequence witnesses preserve the existing gap and
completeness rules. Missing body events remain unknown; terminal success, process exit, or a later
pass never fabricates an earlier node outcome. An early exit marks the unvisited suffix of that pass
as NotRun only when the observed pass boundary proves it.

## Rollout and compatibility

The new definition version is required for Loop constructs; old definitions continue to compile and run. Existing standalone binaries retain their old description and observation versions. A new CLI accepts both protocol generations and chooses the corresponding reducer. Generated Loop runners are not installed until all body validation and protocol checks pass. Compile errors leave an existing executable and adjacent lock intact.

The first delivery includes sequential Loop execution, typed overwrites, condition/max/explicit exit, nesting within the depth limit, and observation. It does not import Dify YAML, implement array iteration or parallel execution, add conversation variables, add assignment operations beyond overwrite, add error-continue modes, or provide automatic rollback of plugin side effects.
