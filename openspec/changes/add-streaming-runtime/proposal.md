# Proposal

## Why

A workflow processing a sequence needs an instance lifetime longer than one message. Event providers
must retain state while message outputs remain isolated, with timer progress and bounded resource use.

## What Changes

- Add an opt-in stream schema, a typed input source, and message-domain validation.
- Add in-memory instances with independent admission and output consumption.
- Keep scheduling, timers, capacity reserves, drain, and cancellation in one runtime implementation.
- Reject unsupported standalone generation, stream observation, and snapshot capture explicitly.

## Impact

Single-run workflows retain their existing behavior. Event fixtures validate the runtime independently
of any collection policy. Built-in Batch, standalone transport, and stream observation follow separately.
