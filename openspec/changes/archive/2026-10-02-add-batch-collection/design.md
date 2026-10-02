# Design

`mfn-core` owns the collection policy. Its factory creates event state and metadata with `item` and
`items` ports. `OutputDerivation::CollectInput` resolves the output as `List(T)` without treating a
single input's literal value as evidence for the whole batch.

The first accepted item establishes a monotonic deadline. Later items do not extend it. An input at or
after the deadline first seals the old buffer, then starts a new one. Count thresholds seal at
`len >= max_items`. Timer callbacks flush only when due; upstream closure flushes a nonempty tail.
Empty buffers emit nothing, and sealing cancels the old deadline.

Values retain their immutable handles. Logical retained bytes participate in runtime accounting.
Failure and cancellation remain runtime policies and do not flush a tail. The node has no task
execution method, scheduler, worker pool, or transport implementation.

Controlled-clock node tests establish deadline boundaries. Integration tests verify collection type
inference, whole-batch outputs, partial tails, explicit package selection, and conditional skips.
Flush metadata is introduced together with its observation consumer in a later change.
