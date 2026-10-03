# Design

## Shared value sizes

An immutable value allocation owns its compact JSON length cache. Zero denotes an unmeasured value;
every valid JSON encoding is nonempty. Concurrent measurements may duplicate initial work but can
only publish the same exact length. A failed bounded traversal never stores a partial length.
Arrays and objects compose cached child lengths, JSON delimiters, and escaped key lengths. Number
formatting follows serde_json. Snapshot aliases refer to the same shared allocation as the cache.

Typed value accounting uses this cache. Payload-limit errors carry the applicable limit; actual
encoding failures retain their serde_json source. A shared value is still charged once per binding.

## Transport

A bounded buffer accepts each serialization write only while the record fits its limit. The runtime
writes the buffer to the output descriptor only after serialization succeeds. It keeps the existing
line-size checks, resource-error classification, and acknowledged delivery behavior.

## Incremental context accounting

A frame budget records the retained-byte total alongside its limits. Publication removes replaced
payload charges using cached value lengths, retains existing key charges, and adds new bindings.
It validates the tentative total before committing values and accounting together, without scanning
unchanged outputs or maintaining a second binding map. Scope guards save and restore the budget
with the output map. Every task publication remains checked before downstream execution.

Queued emissions retain the context already prepared for validation. Promotion attaches frame limits
and observation while retaining the existing credit reservation and message-sequence ordering.

## Validation and ablation

Compare lengths against serde_json across escaping, numbers, shared subtrees, and changing limits.
Exercise atomic failed publication, replacement, skip, scope unwind, and intermediate oversize errors.
Compare baseline, cached sizing, and incremental accounting on forwarding chains and Batch flows.
Keep benchmark sources and reports under the ignored target directory.
