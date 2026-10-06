# Unlock Stream Event Callbacks

## Why

The coordinator invokes EventNode methods while holding the scheduler mutex. A callback that cancels its workflow synchronously wakes FailureWake, which tries to acquire that same mutex and deadlocks.

## What Changes

- Hand the existing scheduler guard across event invocations explicitly and release it during plugin execution and event reporting.
- Keep event invocation serial and restore coordinator-owned operator state before committing effects or returning an error.
- Reject effects after cancellation and preserve the first recorded failure.
- Keep focused regressions for callback cancellation, buffered inspection, concurrent state access, and cancellation during event reporting.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-stream-execution`: require event callbacks to allow synchronous cancellation without holding the scheduler mutex.

## Impact

The change affects the stream coordinator and compiler integration tests. The public EventNode contract retains its existing Send requirement and serial invocation model.
