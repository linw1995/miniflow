## Producer/receiver split

Keep the runtime recorder stateless. The retention decision belongs to the caller-owned
`SnapshotRecording`, separating the producer-side snapshot encoder from the replay store.
`SnapshotRetention::Unbounded` remains the default and `with_sink` delegates to it, so
existing callers observe no change. `with_sink_and_retention` accepts
`SnapshotRetention::Bounded { max_values }` and installs an LRU-bounded live deduplication
dictionary on the producer.

## Eviction and redefinition

Bounded mode evicts the least-recently-used live dictionary entry when the limit is
exceeded. When an evicted value reappears, the producer defines it again under a new
value ID, emitting a repeated `Value` wire record. `value_count()` continues to count
total assigned IDs, so it grows across redefinitions. The wire format, snapshot protocol
version, consumer `SnapshotStore`, and the `current()`/`history()` contracts are
unchanged: the receiver admits the redefinition as an ordinary definition and retains
the complete stream.

## Runner wiring

The generated runner template reads `MF_SNAPSHOT_DICTIONARY_LIMIT` (exported as
`mf_telemetry::SNAPSHOT_DICTIONARY_LIMIT_ENV`) when constructing the OTLP snapshot
recording. A valid `usize` enables bounded retention; an absent variable or an invalid
value retains unbounded retention, with invalid values reported through stderr. TUI and
programmatic callers keep choosing retention through the recording constructor.

## Verification and review

Extend existing snapshot tests to cover bounded eviction, redefinition under new IDs,
receiver reconstruction parity with `value_count()`, and env wiring fallbacks. Run
pinned-toolchain tests and required hooks, then synchronize and archive the completed
specification.
