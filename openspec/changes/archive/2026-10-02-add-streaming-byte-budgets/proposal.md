# Proposal

## Why

Message-count limits bound scheduling queues but do not bound the size of values retained by an
instance. Keep byte budgeting separate from streaming lifetimes, Batch collection policy, transport,
and observation so its accounting costs and limits can be reviewed independently.

## What Changes

- Add payload and retained-value byte limits to streaming execution settings.
- Account for queued inputs, message contexts, event-node buffers, pending emissions, and delivery.
- Reserve capacity for active frames and event handoffs so byte pressure can reach input admission.
- Apply the same limits to Batch state and generated JSON Lines transport.
- Preserve the existing serialization-based accounting design in this separate change.

## Impact

The parent streaming stack works with message-count and worker limits alone. This change adds byte
budgets, the event retained-value reporting hook, transport size checks, and byte-specific tests.
Logical byte accounting does not measure process RSS or arbitrary plugin allocations. Per-publication
context scans remain a performance tradeoff for this PR's separate review.
