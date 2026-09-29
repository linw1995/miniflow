# Design

## Context

The compiler emits direct node calls in topological order, while the runtime context stores one outcome per qualified node output. Reusing that context across items would leak results. The observation contract also permits only one lifecycle per described node and run.

The target behavior follows the [Dify Iteration documentation](https://docs.dify.ai/en/cloud/use-dify/nodes/iteration): an array body can run sequentially or with at most ten concurrent items, and item errors can terminate, contribute null, or be removed.

## Decisions

### 1. Treat Iteration as orchestration

`builtin.iteration` is recognized by the compiler and runtime, not resolved through a node package. Its configuration contains `mode`, `on_error`, and a body with ordinary nodes, data/control edges, and one required result selection. Outer edges connect only to `items` and `results`.

The compiler inserts `@iteration` into the body graph. Its `items` output has type `Any` and its `index` output has type `Int64`. Body nodes use normal dependency, context-reference, port, and cycle checks. The body cannot define the reserved ID or contain an iteration or internal input kind. The input element type remains a checked boundary because a broad input array need not have homogeneous elements.

### 2. Reuse prepared nodes, isolate invocation data

An Iteration node owns a prepared body. Each item gets a fresh child execution context seeded with the item and index. Body node instances are shared across invocations, matching the existing `Node: Send + Sync` contract. Parallel mode starts at most ten workers. A failure stops new scheduling under `terminate`; already started items finish before the node returns. Successful results and reported failures are ordered by original input index.

The body result is selected as a required port. A skipped selection becomes an item error. `terminate` publishes no partial array. `continue_on_error` inserts JSON null for failed positions, while `remove_failed` drops them. The output type is `List(T)` for terminate/remove-failed and `List(Any)` for continue-on-error because null may appear.

### 3. Generate the body as direct calls

The embedded plan retains the body configuration. Runner validation constructs and checks all body nodes, including for empty arrays. Generated Rust prepares body nodes once, captures them in the Iteration node, and calls the shared context execution helper in a per-item closure. It does not construct a `Flow` or interpret the body graph during execution. The in-memory compiler constructs a `Flow` from the same validated body and uses the same execution and output-selection helpers.

Body node and edge order is normalized in the compiled plan. Changing configuration or dependencies regenerates the runner and preserves the existing validated-install boundary.

### 4. Report repeated execution without changing the outer lifecycle contract

The runner description contains the outer `builtin.iteration` node and its outer edges. Its version 1 lifecycle spans
the complete array operation. Repeated items and body nodes emit separate OTel logs in the `mf.iteration` scope,
leaving the bounded `mf.workflow` lifecycle sequence unchanged. Every detail record identifies the workflow, run,
outer iteration node, and item index; body-node records also identify the inner node and kind. Failed items remain
visible when the outer node succeeds under a continue policy.

Each item span is a child of the outer Iteration node span, and each body-node span is a child of its item span. The item context is attached on its worker thread so plugin spans created during a body-node invocation inherit the body-node context. The terminal UI continues to show the outer node; displaying repeated body invocations there requires an occurrence-aware UI state model.

## Validation

Cover empty and nonempty arrays, index access, order under parallel execution, the ten-worker ceiling, all error policies, invalid body structure and result ports, generated runner execution, and description output. Run the repository formatting, pre-commit, and flake checks.
