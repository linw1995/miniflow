# Proposal

## Why

Domain work currently runs through three independent mechanisms: scoped threads for oneshot flows, a worker pool for stream domains, and per-Iteration threads coordinated by a separate permit budget. A shared bounded worker pool can enforce one worker limit across these paths and remove duplicated scheduling machinery.

## What Changes

- Route oneshot and stream execution-domain jobs through the same runtime worker-pool implementation.
- Route parallel Iteration item work through that pool and remove the separate domain permit counter.
- Let a pool worker help execute queued nested work while waiting for its children, so nested scheduling progresses without creating extra worker threads.
- Keep each blocking stream producer on its dedicated bounded lane so producer backpressure cannot consume domain-worker capacity.
- Preserve existing runtime and stream worker limits, domain ordering, scope semantics, and Iteration result order.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `runtime-worker-pool`: support cooperative execution of nested jobs while keeping all handler work within the configured worker bound.

## Impact

- `mf-runtime` worker pool, execution context, oneshot domain scheduler, and stream scheduler.
- `mfn-core` parallel Iteration scheduling.
- Worker-pool, runtime, stream, and Iteration tests plus worker-pool documentation.
