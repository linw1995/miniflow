# Proposal

## Why

Repeated task invocations and timer-driven emissions cannot use finite-run event identity or retain an
unbounded event history. Streaming needs message-aware observation with independent failure isolation.

## What Changes

- Add a versioned stream event protocol with invocation, domain, message, and nested-scope identity.
- Observe buffering, Batch flushes, acknowledged completion, and failure cleanup.
- Bound retained observation state and preserve legacy protocol decoding.
- Connect in-memory instances and standalone runners to optional stream observation.

## Impact

Observation remains independent of scheduling and collection policy. Existing finite-run telemetry,
plugin tracer ownership, and snapshot wire formats remain compatible. Stream snapshots and terminal
launch remain explicitly unsupported.
