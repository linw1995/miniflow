## ADDED Requirements

### Requirement: Restore compatible default-branch mappings

CI SHALL restore per-test coverage mappings and their companion commit anchor from the default branch. Restore and save SHALL use the same source-input fingerprint, scoped by host platform. Generated fixture, vendor, and local environment manifests MUST NOT change cache identity. PRs MUST NOT publish replacement mappings.

#### Scenario: Collection creates generated manifests

- **WHEN** a successful collection creates Cargo manifests in generated directories
- **THEN** its saved baseline remains compatible with a clean checkout using the same source manifests and pinned build inputs

#### Scenario: A source manifest changes

- **WHEN** a workspace manifest, lockfile, or pinned build input changes
- **THEN** CI does not assume the old database is compatible and retains full-suite fallback

### Requirement: Refresh mappings after successful main verification

Main-branch CI SHALL execute incremental collection when the database is compatible and inputs support selective execution. It SHALL retain untouched tests' mappings and original source coordinates. Missing data, incompatible fingerprints, unavailable anchors, or untracked inputs SHALL require full collection. Only successful collection SHALL advance the companion commit anchor and publish the baseline.

#### Scenario: A Rust source change affects part of the suite

- **WHEN** main changes Rust source covered by a compatible database
- **THEN** CI collects affected tests once and preserves mappings for untouched tests without repeating aggregate coverage execution

#### Scenario: A collection fails

- **WHEN** compilation or a selected test fails during collection
- **THEN** the companion anchor remains unchanged and CI does not publish a replacement baseline

### Requirement: Apply conservative input fallbacks

Selection SHALL consider changes since collection rather than only the current PR diff. Build scripts, proc macros, fixtures, snapshots, examples, and unknown non-Rust inputs SHALL force full execution. Known repository documentation SHALL not independently force a full suite. Diagnostics SHALL identify fallback inputs and collection mode.

#### Scenario: Rust changes include OpenSpec documentation

- **WHEN** a change modifies Rust source and OpenSpec Markdown or metadata
- **THEN** documentation alone does not force the full suite

#### Scenario: A Markdown fixture changes

- **WHEN** a change modifies Markdown used as a fixture or an unknown input
- **THEN** CI retains full-suite execution

### Requirement: Separate partial and complete coverage reporting

PRs SHALL retain patch coverage from selected tests. Main mapping collection SHALL not publish aggregate partial reports as complete project coverage. Nightly SHALL retain full-platform verification and complete Codecov reports. Incremental collection MUST NOT relabel untouched mappings as freshly collected coverage.

#### Scenario: Main performs incremental collection

- **WHEN** main refreshes only affected tests
- **THEN** it publishes mappings while complete project coverage remains the responsibility of full coverage runs

### Requirement: Support explicit local affected-test iterations

Local development SHALL support clean-commit baseline collection, affected execution after source edits, and compatible incremental refresh after committing. Collection and execution SHALL use the same pinned toolchain, host platform, and instrumentation settings. Developers SHALL be able to reuse a persistent generated-project cache. Generated mappings and reports SHALL remain under ignored target directories, and affected execution SHALL not replace required final validation.

#### Scenario: Repeat a local Rust edit

- **WHEN** a developer edits Rust source after collecting a compatible local baseline
- **THEN** the explicit affected command selects relevant tests and can reuse prepared fixture dependencies
