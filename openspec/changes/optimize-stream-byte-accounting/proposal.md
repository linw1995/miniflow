# Proposal

## Why

Byte checks repeatedly serialize shared values, scan every retained output after each publication,
and serialize transport output twice. Long task chains accumulate quadratic counting work.

## What Changes

- Cache exact compact JSON lengths on immutable shared values and compose container lengths.
- Encode output once into a bounded buffer before writing a complete JSON Lines record.
- Maintain context byte totals incrementally, including replacement, skip, and scope restoration.
- Reuse prepared emission contexts when handing them to downstream frames.

## Impact

Changes affect runtime values, snapshot aliases, context accounting, and stream transport. Logical
per-binding charges, publication checks, reservations, message order, and node lifetimes stay intact.
No full JSON buffers or global history caches are retained.
