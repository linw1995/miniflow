# Proposal

## Why

Encoded JSON length does not represent retained Rust values: small scalars require allocations and
containers retain spare capacity. Byte checks also repeatedly traverse shared values and retained
contexts. Long task chains accumulate quadratic counting work.

## What Changes

- Cache Rust heap estimates on immutable shared values, including container capacity and children.
- Separate the JSON record limit from message and instance memory budgets.
- Retain exact compact JSON length caching only for input record validation.
- Encode output once into a bounded buffer before writing a complete JSON Lines record.
- Maintain context byte totals incrementally, including replacement, skip, and scope restoration.
- Reuse prepared emission contexts when handing them to downstream frames.

## Impact

Changes affect runtime values, snapshot aliases, context accounting, Batch retention, and transport.
`max_record_bytes` limits encoded records; `max_message_bytes` and `max_buffered_bytes` limit
estimated retained memory. Shared allocations are charged per retaining reference. Reservations,
message order, and node lifetimes stay intact; the budget is not an RSS measurement.
