## Why

Construction failures currently escape through execution error types, and generated oneshot execution functions construct their own nodes and Flow. This makes a successful build insufficient to establish the runtime contract.

## What Changes

- Separate construction errors from runtime execution errors.
- Move preparation off FlowRuntime and generate explicit preparation entry points.
- Require generated execution APIs to receive prepared Flows.
- Keep compiler convenience APIs explicit about preparation and execution failures.
- Preserve source chains, preparation telemetry, and existing execution behavior.

## Capabilities

### Modified Capabilities

- `workflow-runtime-execution`: require complete validated plans at execution entry points.
- `node-preparation`: separate construction errors from execution errors.

## Impact

Runtime APIs, compiler adapters, generated runners, their existing tests, and documentation.
