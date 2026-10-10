## Why

Fixed builtin event and stream nodes still bind input/output types through a handwritten `NodePortContract` while
business methods receive dynamic maps. Preparation cannot obtain that binding from their execution interfaces.

## What Changes

- Add typed event and producer execution contracts whose associated input/output types also drive port reflection.
- Decode input events and producer inputs before business execution and encode typed outputs at runtime boundaries.
- Reuse existing event containers with dynamic defaults and use a borrowed typed producer callback.
- Migrate Batch and Readline to typed execution without handwritten ports, decoding, or encoding.
- Preserve dynamic provider interfaces, state ownership, backpressure, resource requirements, and type evidence.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `node-preparation`: prepare typed event and stream executors directly from their execution-associated contracts.
- `typed-port-contracts`: require fixed builtin factories to use automatic typed preparation.
- `stream-node-execution`: adapt typed incremental producers through the existing emission boundary.

## Impact

Changes affect `mf-runtime`, fixed builtin event/stream providers, tests, and node development documentation.
Existing dynamic event containers retain their defaults. No new workflow JSON fields, dependencies, or execution kinds
are introduced. Shared conversion diagnostics identify typed nodes rather than only typed tasks.
