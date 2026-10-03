# Design

## Shared value sizes

Each immutable allocation stores an eagerly composed heap estimate. Scalars include the value
allocation and an Arc header allowance. Strings include their Arc allocation and UTF-8 bytes.
Arrays include their allocated capacity and cached child estimates. Objects include keys, child
estimates, and a per-entry tree allocation estimate; the standard library does not expose node
capacity. Allocator overhead and exact process RSS are outside this estimate.

The estimate excludes the outer ValueRef handle, which belongs to the retaining container. Shared
children are charged per reference, without a global identity table. Saturating arithmetic makes
unrepresentable estimates fail finite budgets. Batch includes its vector capacity and releases the
charge when it moves the vector into an emitted array.

Internal accounting reads the heap estimate in constant time. Snapshot aliases refer to the same
allocation as the estimate. Resource-limit errors carry the applicable limit; actual encoding
failures retain their serde_json source.

## Independent limits

`max_record_bytes` bounds JSON input and output records. `max_message_bytes` bounds the sum of
estimated value heaps in each input or node publication. Both default to 1 MiB.
`max_buffered_bytes` defaults to 64 MiB and covers retained values plus runtime metadata allowances.
Memory reservations use the message memory limit and fixed scalar layouts. A record limit cannot
provide a memory bound for an array with spare capacity, so the limits must remain independent.

## Transport

A bounded buffer accepts each serialization write only while the record fits its limit. The runtime
writes the buffer to the output descriptor only after serialization succeeds. It keeps the existing
line-size checks, resource-error classification, and acknowledged delivery behavior.
Input records are measured from their received bytes before parsing. In-process admission applies
the memory limit. A separate JSON length cache and counting traversal are therefore unnecessary.

## Incremental context accounting

A frame budget records the retained-byte total alongside its limits. Publication removes replaced
payload charges using cached heap estimates, retains existing key charges, and adds new bindings.
It validates the tentative total before committing values and accounting together, without scanning
unchanged outputs or maintaining a second binding map. Scope guards save and restore the budget
with the output map. Every task publication remains checked before downstream execution.

Queued emissions retain the context and budget already prepared for validation. Promotion attaches
observation while retaining the existing credit reservation and message-sequence ordering.

## Validation and ablation

Check that memory charges reflect vector capacity, nested values, shared children, and Batch handoff.
Exercise atomic failed publication, replacement, skip, scope unwind, and intermediate oversize errors.
Compare the original JSON accounting, cached memory estimates, and incremental accounting on
forwarding chains and Batch flows. Compare the latter two separately to isolate context scanning.
Keep benchmark sources and reports under the ignored target directory.
