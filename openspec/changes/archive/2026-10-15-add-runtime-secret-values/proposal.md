## Why

Runtime variables currently reach opt-in OTel snapshots as complete JSON values.
Configuration templating in #145 needs a sensitivity marker that survives value
routing before templates are introduced.

## What Changes

- Add immutable secret metadata to ValueRef, recursive marking, and explicit inheritance for derived values.
- Redact secret subtrees before snapshot callbacks, recording, dictionary encoding, and OTel export.
- Redact Display and Debug while preserving business accessors and JSON serialization.
- Preserve metadata-only Loop scope updates across execution domains.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-runtime-execution`: secret value semantics and propagation.
- `workflow-observability`: secret-safe snapshot boundaries and diagnostics.

## Impact

Runtime values, scope merging, snapshot callbacks, focused tests, and documentation.
No dependency, wire version, template syntax, or global node-output taint policy is added.
Issue #145 remains open for configuration templating.
