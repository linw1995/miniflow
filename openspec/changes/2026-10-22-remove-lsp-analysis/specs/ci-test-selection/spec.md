## MODIFIED Requirements

### Requirement: Execute selected PR suites and complete main suites

PR CI SHALL select package owners and all reverse dependencies from the increment since an eligible successful validation, or the cumulative PR diff otherwise. Known shared consumers and deferred configuration exceptions SHALL remain effective. Unknown inputs SHALL retain full-workspace fallback. Tests SHALL use the pinned toolchain and current merged tree. Main and manual full runs SHALL retain full suites; test failures SHALL prevent successful report publication.

#### Scenario: The event base is older than the tested merge base

- **WHEN** the event payload names an older base SHA than the merge commit's first parent
- **THEN** cumulative PR selection uses the actual first parent, and incremental reuse requires that same actual base identity

#### Scenario: A pull request changes one source file

- **WHEN** the selected comparison changes a package-owned Rust source file or fixture
- **THEN** CI runs that package and all its reverse dependencies with coverage, including tests that passed in an earlier run but are affected again

#### Scenario: A pull request changes only deferred configuration

- **WHEN** the selected comparison only changes known root dependency, toolchain, execution, or CI configuration
- **THEN** its Rust selection is empty, relevant non-Rust checks still run, and main/nightly retain full verification

#### Scenario: No Rust package is selected

- **WHEN** selection is empty
- **THEN** CI skips coverage tests and report uploads without issuing an unfiltered nextest command, while preserving an explicit empty disposition

#### Scenario: A test fails

- **WHEN** compilation or a test fails during coverage execution
- **THEN** CI fails, does not publish a successful coverage result, and does not advance the validation baseline
