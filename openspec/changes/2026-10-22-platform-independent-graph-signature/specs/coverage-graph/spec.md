## MODIFIED Requirements

### Requirement: Index per-test coverage into a versioned commit graph

Successful default-branch coverage execution SHALL collect per-test line coverage and index it into an immutable SQLite graph keyed by commit, binding commit identity, producing run and attempt, schema version, and execution signature. Rows SHALL store compressed line intervals per file per test. Collection SHALL use the producing run's pinned toolchain.

The execution signature SHALL bind a platform-independent toolchain identity (the `release` and `commit-hash` fields of `rustc -vV`) alongside the coverage-relevant environment, feature set, build target, and nextest profile; the host triple SHALL NOT participate, because line-to-test attribution does not depend on the host architecture.
Unparseable toolchain output SHALL be an explicit error rather than a silently weaker signature.
Consumers on a different host remain sound: platform-specific selectors absent from the runnable inventory widen to the recorded baseline, and platform-specific covered lines may only over-select tests.

#### Scenario: Index a successful main run

- **WHEN** a default-branch coverage run completes successfully
- **THEN** the graph for that commit maps each covered executable line interval to the distinct tests that covered it, bound to that run and its execution signature

#### Scenario: Accept a same-toolchain graph from another host

- **WHEN** a graph's signature was produced from `rustc -vV` output with the same release and commit-hash but a different host than the consumer
- **THEN** the consumer treats the toolchain identity as matching

#### Scenario: Reject an incompatible graph

- **WHEN** a graph has a schema version or execution signature incompatible with the consumer, or is malformed
- **THEN** the consumer treats the graph as unavailable with a recorded reason rather than failing selection or excluding tests
