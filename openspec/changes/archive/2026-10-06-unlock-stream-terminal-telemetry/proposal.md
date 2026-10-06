# Unlock Stream Terminal Telemetry

## Why

Dependency failures and skipped stream operators still report telemetry while holding the scheduler mutex. A synchronous log processor that cancels the workflow reenters that mutex and deadlocks.

## What Changes

- Release the scheduler mutex before reporting dependency failures and skipped operators.
- Recheck recorded failure after reacquiring the mutex, before returning a dependency error or continuing scheduling.
- Extend the existing cancellation regression with one scenario for each terminal path.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-stream-execution`: include dependency failure and skip reporting in the scheduler lock boundary.

## Impact

The change affects stream scheduling and cancellation integration tests. It adds no public API or scheduling abstraction.
