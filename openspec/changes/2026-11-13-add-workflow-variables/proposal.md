## Why

Typed assignment is currently available only inside Loop bodies. Ordinary DAGs need invocation-local
mutable values for conditional updates and sequential refinement without introducing a one-pass Loop.

## What Changes

- Add definition version `2026-11-13`, definition-level `variables`, and `workflow.assign`.
- Inject a synthetic `%root` source for the declaration instead of requiring a declaration node.
- Declare typed root state per finite callable Flow, with required literal initial values.
- Reuse staged, validated assignment publication and explicit dependency ordering.
- Keep root state separate from container frames, startup inputs, and observation ancestry.
- Initialize fresh state for every invocation and serialize root-state execution domains.
- Preserve `workflow.loop_assign` and Loop-only exits; support `workflow.assign` in Loop bodies too.
- Freeze root-state relationships in compiled artifacts and generated runners.

## Non-goals

Persistent session variables, cross-Flow mutation, root-state streaming instances, atomic arithmetic
operations, and generalized scope exit are not included. Iteration inputs remain immutable.

## Capabilities

### New Capabilities

- `workflow-variables`: typed mutable root state for finite Flow invocations.

### Modified Capabilities

- `workflow-loop-execution`: generic assignment syntax can target the current Loop variable set.

## Impact

Runtime state, compiler lowering and validation, generated preparation, provider reservation, and
workflow authoring documentation change. No new telemetry frame or lifecycle protocol is needed.
