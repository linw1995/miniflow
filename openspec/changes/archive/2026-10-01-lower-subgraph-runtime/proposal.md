## Why

Loop and Iteration currently place container scheduling policies in the runtime and require the compiler to construct their executors. Sequential Iteration repeats a body just as Loop does; their common execution mechanism should sit below item, state, termination, and concurrency policies.

## What Changes

- Provide shared execution scopes and prepared subgraphs in the runtime.
- Bind prepared bodies through the registered node implementation.
- Move Loop and Iteration execution policies into `mfn-core`, sharing the sequential Loop driver.
- Share container body assembly in memory and generated runners.
- Discover bodies through node-declared subgraph contracts, including third-party containers.
- Let node providers adapt generic scope hooks to their observation protocols.

## Capabilities

### New Capabilities

- `runtime-subgraph-execution`: Scoped execution, declared bodies, prepared-body binding, and observation hooks without container scheduling policies.

### Modified Capabilities

- `workflow-loop-execution`: The registered Loop node owns execution policy and consumes an engine-prepared body.
- `iteration-execution`: The registered Iteration node owns item scheduling and failure policies, using the shared Loop driver for sequential execution.

## Impact

High-level execution constructors move from `mf-runtime` to node implementations. Providers use `SubgraphDefinition`, `PreparedSubgraph`, and `ExecutionScope`, and select a `ScopeObserver` when needed. Standalone compilation prepares registered bodies before emitting final direct calls. Workflow JSON, node kinds, bounds, result semantics, and observation protocols remain compatible.
