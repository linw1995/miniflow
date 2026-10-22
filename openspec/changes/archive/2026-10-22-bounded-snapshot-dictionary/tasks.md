## 1. Add producer-side retention policy

- [x] 1.1 Add `SnapshotRetention` and `SnapshotRecording::with_sink_and_retention`, keeping `with_sink` unbounded and the runtime recorder stateless.
- [x] 1.2 Bound the live deduplication dictionary with LRU eviction, redefine evicted values under new value IDs, and preserve `value_count()` totals, wire format, and store contracts; commit this group.

## 2. Wire generated runner configuration

- [x] 2.1 Export `SNAPSHOT_DICTIONARY_LIMIT_ENV` from `mf-telemetry` and read it in the generated runner template with unbounded fallback and an invalid-value diagnostic.
- [x] 2.2 Verify bounded eviction, redefinition, receiver parity, and env fallbacks; commit this group.

## 3. Review and archive

- [x] 3.1 Update observability documentation, run focused tests and required hooks, and remove unnecessary abstractions.
- [x] 3.2 Review AGENTS.md compliance, synchronize formal specs, and archive in a separate commit.
