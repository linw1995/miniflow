# Proposal

## Why

A workflow needs an explicit collector that preserves item order and emits a whole batch when its
size threshold, first-item deadline, or upstream closure requires a flush.

## What Changes

- Register `builtin.batch` with positive count and timeout configuration.
- Collect input types as `List(T)` while discarding exact-value evidence for the accumulated array.
- Retain ordered value handles and flush full or partial batches through the event contract.

## Impact

Batch uses the bounded in-memory runtime and remains unavailable in synchronous bodies. Ordinary
nodes can consume its complete array output. Standalone transport and flush observation are separate
changes, so this layer contains no new runner or telemetry protocol.
