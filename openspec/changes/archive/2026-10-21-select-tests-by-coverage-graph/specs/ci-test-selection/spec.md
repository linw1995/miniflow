## ADDED Requirements

### Requirement: Publish coverage graphs from default-branch verification

Successful default-branch coverage runs SHALL publish the per-commit coverage graph through the existing artifact pipeline. Publication SHALL follow the same completed-attempt and compatibility rules as validation records, including the graph schema version in execution-signature compatibility. Main/nightly full-suite retention and report ownership SHALL remain unchanged.

#### Scenario: Main coverage publishes a graph

- **WHEN** a default-branch coverage run passes completely
- **THEN** the resulting graph artifact is published with its commit, run, schema version, and execution signature

#### Scenario: Failed runs publish nothing

- **WHEN** a default-branch coverage run fails or is cancelled
- **THEN** no graph is published for that commit and later consumers resolve an earlier compatible ancestor

### Requirement: Execute graph-derived exact selectors

PR CI MAY consume graph-derived exact test selectors from the shared Flow through the existing exact-filter coverage consumer. Selector identities that do not match the executable inventory SHALL widen to the recorded baseline with a diagnostic. Graph-derived selection SHALL NOT weaken the incremental coverage gate, alter baseline advancement, or bypass final all-job validation.

#### Scenario: Run graph-selected tests

- **WHEN** the Flow plan carries graph-derived exact selectors with provenance
- **THEN** CI executes exactly those tests with coverage and evaluates the incremental gate over the same comparison

#### Scenario: Widen on identity mismatch

- **WHEN** a graph-derived selector does not uniquely match the head executable inventory
- **THEN** CI widens that scope to the recorded baseline and reports the mismatch instead of silently dropping tests
