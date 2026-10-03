# Design

`WorkerPool<Job>` owns a bounded `sync_channel` and reusable worker threads. The channel capacity equals
the worker count, matching the current streaming implementation. A shared synchronous callback consumes
each job. The caller owns result delivery and task panic handling, so the pool can serve a streaming
frame without depending on `StreamPlan`, event nodes, or a second result queue.

`try_submit` preserves the rejected job in the standard channel error. The caller can distinguish full
capacity from unavailable receivers. A zero-worker pool accepts no jobs, allowing an event-only stream
to retain its existing startup path.

Dropping the pool closes submissions and joins its threads. Workers finish accepted jobs while their
callbacks return normally. Construction stores each started thread immediately, so a later spawn
failure closes the channel and joins the earlier threads during cleanup. Snafu preserves the original
I/O error and identifies the worker that could not start.

The streaming layer selects the worker count and submits a frame as one job. Its callback advances
consecutive synchronous tasks, records completion, and wakes the coordinator. Existing message ordering,
failure handling, observation context, and byte accounting remain owned by that layer.
