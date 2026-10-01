## Why

The previous reuse work added automatic third-party body discovery and observation adapters beyond the current Loop and Iteration requirements. Their scaffolding costs more than the duplication it removes.

## What Changes

- Defer automatic third-party body discovery and standalone generation.
- Remove runner preparation, artifact caching, and tests specific to that capability.
- Reuse existing observation contexts instead of a scope observer extension framework.
- Retain shared scopes, prepared bodies, and node-owned policies; remove the trivial sequential-loop wrapper.

## Capabilities

### Modified Capabilities

- `runtime-subgraph-execution`: Limit the contract to required execution mechanisms and existing observation contexts.
- `iteration-execution`: Share scoped body execution without requiring a separate loop-driver abstraction.

## Impact

Loop and Iteration workflow behavior remains unchanged. `PreparedSubgraph` remains available for explicit binding. Automatic third-party compilation is deferred.
