# Share Subgraph Workers

## Why

Oneshot execution only owns a worker pool when the top-level plan schedules multiple domains concurrently. Loop passes and sequential Iteration items in a single-domain workflow can therefore create temporary pools repeatedly. Scoped Flow entry points also replace the inherited worker limit with their default options.

## What Changes

- Own one worker pool for each nonempty oneshot Flow execution, including single-domain plans.
- Propagate the existing non-owning handle through domain and body contexts, and retain inherited worker limits in scopes.
- Preserve serial scoped-domain execution and the existing cooperative worker scheduler.
- Retain one regression test for actual worker reuse across Loop passes and sequential or parallel Iteration items.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `runtime-worker-pool`: require run-wide pool ownership and inherited worker settings for prepared subgraphs.

## Impact

The change affects `mf-runtime` Flow execution and compiler integration tests. Workflow schemas, node registration, stream producer lanes, and container execution policies remain unchanged.
