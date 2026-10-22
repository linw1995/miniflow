# coverage-graph Specification

## Purpose

Provide an immutable, versioned per-commit map from workspace source line intervals to the distinct tests that covered them, collected during successful default-branch coverage runs and published through the existing artifact pipeline, so that test selection can narrow package scope to exact covering tests with conservative widening when the graph cannot answer.

## Requirements

### Requirement: Index per-test coverage into a versioned commit graph

Successful default-branch coverage execution SHALL collect per-test line coverage and index it into an immutable SQLite graph keyed by commit, binding commit identity, producing run and attempt, schema version, and execution signature. Rows SHALL store compressed line intervals per file per test. Collection SHALL use the producing run's pinned toolchain.

#### Scenario: Index a successful main run

- **WHEN** a default-branch coverage run completes successfully
- **THEN** the graph for that commit maps each covered executable line interval to the distinct tests that covered it, bound to that run and its execution signature

#### Scenario: Reject an incompatible graph

- **WHEN** a graph has a schema version or execution signature incompatible with the consumer, or is malformed
- **THEN** the consumer treats the graph as unavailable with a recorded reason rather than failing selection or excluding tests

### Requirement: Publish graphs only from trusted default-branch runs

The graph SHALL be published through the existing artifact pipeline alongside validation records, only after the producing default-branch run completes all required coverage successfully. Pull request runs and local executions SHALL NOT publish graphs. Consumers SHALL acquire graph artifacts through the existing lazy, bounded, ancestry-ordered retrieval and authenticated access; no external database service or unauthenticated mirror SHALL be required.

#### Scenario: Publish after complete success

- **WHEN** a default-branch coverage run passes completely
- **THEN** its graph artifact is available to later CI and local planning through existing artifact retrieval

#### Scenario: Reject foreign publication

- **WHEN** a graph artifact originates from a pull request run or an unsuccessful attempt
- **THEN** consumers reject it and resolve an eligible default-branch ancestor instead

### Requirement: Answer changed-range queries conservatively

Given a commit graph and changed line ranges, a query SHALL return the distinct test selectors covering those ranges at that commit. Ranges with no covering test, files absent from the graph without an existing non-executable classification, and files changed between the graph commit and the query base SHALL be reported as unresolved rather than silently excluded. A query SHALL NOT require building or executing tests.

#### Scenario: Resolve covered ranges

- **WHEN** a changed range at the query base maps to covering tests in the graph
- **THEN** the query returns exactly those distinct test selectors with the graph commit as provenance

#### Scenario: Report unresolved changes

- **WHEN** a changed range has no covering test, or its file changed after the graph commit
- **THEN** the query reports that range as unresolved so the caller widens its scope

#### Scenario: Absent graph yields no answer

- **WHEN** no compatible graph exists for any eligible ancestor of the query base
- **THEN** the query reports unavailability with a reason and returns no selectors
