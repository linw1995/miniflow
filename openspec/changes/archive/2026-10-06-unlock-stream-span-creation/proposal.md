# Proposal

## Why

Creating a stream operator callback starts an OpenTelemetry span synchronously. A span processor can cancel the workflow from on_start, whose failure waker reenters the scheduler mutex. Callback creation still holds that mutex in the frame, timer, and upstream-close paths, so receiving outputs and joining can deadlock.

## What Changes

- Create event and producer callback spans outside the scheduler mutex.
- Recheck the first recorded failure after reacquiring the mutex, before invoking or submitting an operator.
- Reuse the existing callback helper and subprocess regression harness, retaining only scenarios with distinct ablation detection.

## Impact

The stream runtime and its cancellation regressions change. Public APIs and event ordering remain unchanged. The workflow-stream-execution specification gains explicit span-creation cancellation guarantees.
