## REMOVED Requirements

### Requirement: Execute the complete Linux suite in regular CI

**Reason**: PR selection is already implemented; this change bounds shared-input exceptions while preserving full main/nightly verification.
**Migration**: Use the selective PR execution requirement.

### Requirement: Publish complete coverage reports

**Reason**: Partial PR reports must not replace complete project coverage.
**Migration**: Use separate partial and complete coverage flags.

## ADDED Requirements

### Requirement: Execute selected PR suites and complete main suites

PR CI SHALL select affected packages and reverse dependencies using the current selector. Package manifests SHALL retain owner scope; documented repository configuration exceptions SHALL defer to main/nightly. Known shared inputs SHALL select consumers, while unknown inputs and analyzer failures SHALL select the workspace. Main and manual runs SHALL execute the full suite. Tests SHALL use the pinned toolchain; failures SHALL prevent report publication.

#### Scenario: A pull request changes one source file

- **WHEN** a pull request changes a package-owned Rust source file or fixture
- **THEN** CI runs that package and its reverse dependencies with coverage

#### Scenario: A pull request changes only deferred configuration

- **WHEN** a PR only changes known root dependency, toolchain, execution, or CI configuration
- **THEN** its Rust selection is empty, relevant non-Rust checks still run, and main/nightly retain full verification

#### Scenario: No Rust package is selected

- **WHEN** selection is empty
- **THEN** CI skips coverage tests and uploads without issuing an unfiltered nextest command

#### Scenario: A test fails

- **WHEN** compilation or a test fails during coverage execution
- **THEN** CI fails and does not upload a successful coverage report

### Requirement: Publish coverage with explicit scope

Partial PR coverage SHALL use the `rust-affected` flag for patch coverage only. Complete reports SHALL use `rust` for project and patch coverage. Uploads SHALL disable automatic discovery and report carry-forward. Only reports from successful execution of the current source tree SHALL be published.

#### Scenario: Main verification succeeds

- **WHEN** main passes complete coverage execution
- **THEN** CI uploads complete project coverage for the current source tree

#### Scenario: A pull request executes a partial suite

- **WHEN** a PR runs selected packages rather than the full workspace
- **THEN** its report contributes to patch coverage without replacing complete project coverage

## MODIFIED Requirements

### Requirement: Cache build artifacts without changing test selection

CI SHALL reuse compatible coverage dependencies and generated-project build
artifacts. The coverage script SHALL remove previous report data and workspace
binaries before execution so stale coverage mappings cannot contribute to
current reports. Missing caches MUST NOT change the selected test scope.

#### Scenario: No compatible build cache exists

- **WHEN** CI starts without a compatible dependency cache
- **THEN** it builds the required binaries and executes the selected suite

### Requirement: Support focused local checks without replacing validation

Local coverage execution SHALL use the pinned toolchain, generate reports
under ignored target directories, and support explicit nextest filters and
persistent generated-project build caches. Focused runs SHALL NOT replace
required repository hooks or scheduled full verification.

#### Scenario: Repeat a local test iteration

- **WHEN** a developer runs the coverage script with an explicit nextest filter
- **THEN** the selected tests execute and compatible build dependencies may
  be reused without requiring a prior coverage database
