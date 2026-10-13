# ci-test-selection Specification

## Purpose

Define complete Rust test execution, coverage reporting, and scheduled
cross-platform verification for repository CI.

## Requirements

### Requirement: Execute the complete Linux suite in regular CI

Pull requests and pushes to main SHALL execute the complete Linux test suite
with the pinned toolchain and generate coverage for the current source tree.
Test execution MUST NOT depend on prior coverage mappings or changed-input
selection. Compilation or test failures SHALL fail CI and prevent report
publication.

#### Scenario: A pull request changes one source file

- **WHEN** a pull request changes a Rust source file, fixture, or build input
- **THEN** CI runs the complete Linux test suite before uploading coverage

#### Scenario: A test fails

- **WHEN** compilation or a test fails during coverage execution
- **THEN** CI fails and does not upload a successful coverage report

### Requirement: Cache build artifacts without changing test selection

CI SHALL reuse compatible coverage dependencies and generated-project build
artifacts. The coverage script SHALL remove previous report data and workspace
binaries before execution so stale coverage mappings cannot contribute to
current reports. Missing caches MUST NOT prevent complete test execution.

#### Scenario: No compatible build cache exists

- **WHEN** CI starts without a compatible dependency cache
- **THEN** it builds the required binaries and executes the complete suite

### Requirement: Publish complete coverage reports

PR, main, and nightly coverage runs SHALL upload the current complete LCOV
report under the same rust flag with automatic report discovery disabled.
Codecov SHALL evaluate project coverage and PR patch coverage using that flag
without carrying forward prior reports.

#### Scenario: Main verification succeeds

- **WHEN** main passes complete coverage execution
- **THEN** CI uploads complete project coverage for the current source tree

### Requirement: Retain scheduled cross-platform full verification

Nightly SHALL run complete Linux and macOS flake checks and complete Linux
coverage. Scheduling SHALL retain daily recent-change verification, retries
for commits without successful full verification, and at least weekly full
verification. Skipped, failed, or partial suites MUST NOT reset the weekly
deadline. Manual dispatch SHALL always run the full suite.

#### Scenario: The branch is quiet after successful verification

- **WHEN** the current commit passed the full nightly suite within the last
  seven days and the default branch has no commits in the last 24 hours
- **THEN** the scheduled workflow may skip full verification

#### Scenario: Manual verification is requested

- **WHEN** a user manually dispatches nightly
- **THEN** Linux and macOS full checks and complete Linux coverage execute

### Requirement: Support focused local checks without replacing validation

Local coverage execution SHALL use the pinned toolchain, generate reports
under ignored target directories, and support explicit nextest filters and
persistent generated-project build caches. Focused runs SHALL NOT replace
required repository hooks and complete flake validation.

#### Scenario: Repeat a local test iteration

- **WHEN** a developer runs the coverage script with an explicit nextest filter
- **THEN** the selected tests execute and compatible build dependencies may
  be reused without requiring a prior coverage database
