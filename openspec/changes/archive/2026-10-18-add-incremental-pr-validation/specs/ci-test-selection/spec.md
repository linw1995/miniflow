## MODIFIED Requirements

### Requirement: Execute selected PR suites and complete main suites

PR CI SHALL select package owners and all reverse dependencies from the increment since an eligible successful validation, or the cumulative PR diff otherwise. Known shared consumers and deferred configuration exceptions SHALL remain effective. Unknown inputs and analyzer failures SHALL retain full-workspace fallback. Tests SHALL use the pinned toolchain and current merged tree. Main and manual full runs SHALL retain full suites; test failures SHALL prevent successful report publication.

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

### Requirement: Publish coverage with explicit scope

PR CI SHALL gate coverage over the same diff used for test selection and publish its range and current report as CI artifacts. Incremental reports SHALL NOT be represented as cumulative PR coverage or uploaded to a cumulative patch gate. Full main/nightly reports SHALL use `rust` for project coverage. Automatic report discovery and carry-forward SHALL remain disabled. Only successful current-source execution SHALL publish successful coverage results.

#### Scenario: Main verification succeeds

- **WHEN** main passes complete coverage execution
- **THEN** CI uploads complete project coverage for the current source tree

#### Scenario: A pull request executes a partial suite

- **WHEN** a PR runs selected packages for an increment
- **THEN** its report is evaluated against that increment and retained as a scoped artifact without replacing complete project coverage or asserting cumulative PR patch coverage

## ADDED Requirements

### Requirement: Reuse only compatible successful validation

A reusable baseline SHALL identify the same repository, PR, actual target-base commit, and compatible execution policy. Its PR head SHALL be an ancestor of or equal to the current PR head. Its exact producing workflow attempt SHALL have completed successfully with a valid matching record. Among eligible records, CI SHALL choose the nearest ancestor by commit history, not completion time.

#### Scenario: Continue after a successful push

- **WHEN** A passed all required checks and a compatible descendant B is pushed
- **THEN** selection compares the tested merge snapshots for A and B

#### Scenario: Continue after an unsuccessful push

- **WHEN** A succeeded, B failed or was cancelled or remains pending, and C descends from B
- **THEN** selection compares the tested merge snapshots for A and C

#### Scenario: Runs complete out of order

- **WHEN** successful B descends from successful A but A completed later
- **THEN** a descendant C reuses B rather than A

#### Scenario: Retry a validated head

- **WHEN** the same PR head is retried with identical merged source and compatible execution policy
- **THEN** the incremental source selection can be empty and the result records which successful validation was reused

### Requirement: Reset reuse when evidence is unavailable or incompatible

CI SHALL fall back to the cumulative PR affected scope with a reason when no eligible record is available. A reset SHALL NOT itself force the entire workspace or remove documented configuration deferrals. Missing current-source identities SHALL fail validation. Failed, malformed, expired, foreign, or mismatched records SHALL NOT authorize test exclusion.

#### Scenario: First validation or unavailable history

- **WHEN** this PR has no compatible record, or record discovery/history retrieval cannot complete within its bounds
- **THEN** CI compares the actual merge parent to the current merge and reports why reuse was unavailable

#### Scenario: Target branch or policy advances

- **WHEN** the actual target-base commit or effective validation policy differs from the record
- **THEN** CI uses cumulative PR affected scope and establishes a new baseline only after success

#### Scenario: Rewrite PR history

- **WHEN** a force push removes the recorded PR head from current ancestry
- **THEN** that record is rejected even if its run completed successfully

#### Scenario: A recorded merge snapshot is unavailable

- **WHEN** an otherwise eligible record refers to a merge object that cannot be obtained
- **THEN** CI falls back to cumulative PR affected scope instead of substituting another source tree

### Requirement: Compare the actual tested snapshots

Incremental selection SHALL directly compare the previous validated merge tree to the current tested merge tree after validating their common actual target base and PR-head ancestry. It SHALL NOT replace the previous merge snapshot with a merge base. Analysis, compilation, and execution SHALL use the current tested merge tree.

#### Scenario: Successive merge commits are siblings

- **WHEN** the previous and current temporary merge commits share a first parent but neither merge commit is an ancestor of the other
- **THEN** selection uses their direct tree diff and excludes already validated PR changes

### Requirement: Evaluate incremental executable-line coverage

The PR coverage gate SHALL intersect current executable-line mappings with current-side changes in the selected comparison. It SHALL report covered and missed lines and enforce one explicit configured target. Migration SHALL set that target no lower than the effective existing PR requirement. The target SHALL be recorded in validation compatibility and SHALL NOT decrease automatically with selection size.

#### Scenario: Cover the new increment

- **WHEN** A was validated and B changes executable lines
- **THEN** the gate evaluates those changed lines using B's measured coverage and does not demand rerunning unchanged A changes merely to rebuild cumulative coverage

#### Scenario: Insufficient incremental coverage

- **WHEN** the measured ratio is below the configured target
- **THEN** validation fails and A remains the reusable baseline

#### Scenario: New or renamed source

- **WHEN** a current-side changed executable line belongs to a new or renamed file
- **THEN** its current path and line identity participate in the coverage denominator, including zero-hit lines

### Requirement: Distinguish absent coverage from no executable change

Missing, malformed, stale, or unmappable expected coverage SHALL fail validation. Missing LCOV entries SHALL NOT alone prove that changed source is non-executable. Confirmed increments without current changed executable lines SHALL report a not-applicable result rather than fabricated full coverage. Deferred-only empty selections SHALL explicitly skip Rust coverage.

#### Scenario: Expected report is missing

- **WHEN** affected Rust tests require a coverage report but the report is absent or malformed
- **THEN** the gate fails and no successful validation record is published

#### Scenario: Changed source has no mapping

- **WHEN** a changed Rust file is absent from executable mappings and has no independently supported non-executable classification
- **THEN** validation reports the uncertainty and fails instead of silently excluding the file

#### Scenario: Change a declaration-only module entry point

- **WHEN** an unmapped file contains only independently recognized non-executable imports and out-of-line module declarations
- **THEN** those declarations do not enter the coverage denominator, while affected package tests still run and unknown attributes or executable items remain rejected

#### Scenario: Delete code without adding executable lines

- **WHEN** the increment deletes source but has no current changed executable lines
- **THEN** affected tests still run and coverage reports a confirmed zero denominator as not applicable

#### Scenario: Change only deferred documentation

- **WHEN** the increment changes only deferred documentation or configuration
- **THEN** Rust tests and coverage are explicitly skipped while applicable checks remain required

### Requirement: Advance the baseline only after complete validation

A success record SHALL bind source identities, comparison provenance, execution policy, coverage disposition, and producing run/attempt. It SHALL be published only after all required checks pass. Later reuse SHALL verify the final workflow attempt succeeded. Tests or coverage passing alone SHALL NOT advance the baseline, and publication failure SHALL prevent successful final validation.

#### Scenario: Build check fails after coverage succeeds

- **WHEN** coverage passes but another required check fails
- **THEN** the run produces no reusable successful validation

#### Scenario: Workflow fails after artifact publication

- **WHEN** an artifact exists but the producing attempt later fails or is cancelled
- **THEN** future runs reject that record

#### Scenario: Empty increment passes applicable checks

- **WHEN** selection is explicitly empty and all applicable checks succeed
- **THEN** the completed successful run can publish a record preserving the inherited validation identity and empty coverage disposition

### Requirement: Expose comparison scope and support cumulative reruns

CI SHALL report the reused validation identity, resolved comparison, selection reasons, fallback scope, and incremental coverage result. An explicit cumulative-PR rerun SHALL ignore reuse and apply the cumulative comparison to both selection and coverage on the same current merge snapshot. No automatic widening loop SHALL be required.

#### Scenario: Diagnose insufficient coverage

- **WHEN** a developer requests cumulative PR validation after an incremental coverage failure
- **THEN** CI executes cumulative PR affected scope with matching coverage range and publishes a replacement baseline only if all checks succeed

### Requirement: Migrate the required coverage gate coherently

The incremental validation check SHALL replace cumulative PR patch gating when incremental execution is enabled. Required-check configuration SHALL match the new producer; retirement of an old check SHALL neither silently permit merges without the replacement nor leave PRs waiting for an obsolete status. Full main/nightly verification SHALL remain independent of incremental success.

#### Scenario: Enable incremental PR validation

- **WHEN** incremental execution becomes the active PR path
- **THEN** the new validation status supplies the required PR coverage decision and cumulative patch coverage is not simultaneously enforced against an incremental report
