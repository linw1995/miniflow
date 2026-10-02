# Design

## Ownership and message domains

Schema `2026-10-02` opts into streaming through `execution`. The engine supplies `%input.item` and
requires an explicit activation path for every root node. Ordinary tasks preserve message identity;
event emissions start a new domain. Dependencies, context references, and selected outputs must respect
those boundaries.

An instance owns its prepared executors, deadlines, and retained state until drain or termination.
`PreparedStream` separates mutable event state from the immutable `StreamPlan`. Workers share task
executors, while the coordinator invokes events serially. Each frame has isolated outputs, skips, and
a fresh step budget. Synchronous bodies remain task-only.

## Progress and resource bounds

One FIFO frame executes per domain. A bounded worker pool keeps synchronous business calls off the
coordinator, allowing idle deadlines and cancellation to progress. Each operator has one replaceable
deadline, cleared before timer delivery.

Admission, running contexts, retained event values, sealed emissions, and pending outputs are charged
to finite budgets. Preparation reserves capacity for downstream progress and rejects impossible
configurations. No flush needs another host admission permit. Plugins report logical retained bytes;
these limits do not describe process RSS or arbitrary plugin allocations.

## Completion and failure

Input close stops admission, then propagates after admitted work and prior emissions reach downstream
nodes. Successful completion waits for output acknowledgement. Failure and cancellation discard
unstarted work and suppress later publications while waiting for started synchronous calls to finish.
They preserve already delivered results and do not retry or flush a tail automatically.

## Independent delivery

The in-memory API is complete in this change. Runner generation, description, and stream observation
reject unsupported requests until their corresponding implementation is introduced. Snapshot capture
is rejected before startup because its existing value interner retains whole-run history.

Shared test fixtures exercise event retention, deadlines, capacity, and closure without depending on
the built-in Batch node or collection type inference.
