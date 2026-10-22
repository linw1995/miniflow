# ci-test-selection Specification

## Purpose

Define affected PR test selection, full main/nightly execution, scoped coverage
reporting, and scheduled cross-platform verification for repository CI.

## Requirements

### Requirement: Cache build artifacts without changing test selection

CI SHALL reuse compatible coverage dependencies and generated-project build
artifacts. The coverage script SHALL remove previous report data and workspace
binaries before execution so stale coverage mappings cannot contribute to
current reports. Missing caches MUST NOT change the selected test scope.

#### Scenario: No compatible build cache exists

- **WHEN** CI starts without a compatible dependency cache
- **THEN** it builds the required binaries and executes the selected suite

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
required repository hooks or scheduled full verification.

#### Scenario: Repeat a local test iteration

- **WHEN** a developer runs the coverage script with an explicit nextest filter
- **THEN** the selected tests execute and compatible build dependencies may
  be reused without requiring a prior coverage database

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
- **THEN** CI may try another compatible successful ancestor, and otherwise falls back to cumulative PR affected scope without substituting an unverified source tree

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

### Requirement: Select tests by executing the shared Flow

PR CI SHALL obtain its selection by running the shared compiled test-affected Flow through `mf` with the CI branch selected. Adapters MAY transport parameters, summaries, and artifacts but SHALL NOT independently implement file classification, baseline discovery, or test-selection policy. Flow execution failure SHALL fail selection rather than become an empty successful plan.

#### Scenario: Preserve the CI handoff

- **WHEN** the Flow returns a valid package-level plan and comparison provenance
- **THEN** CI passes the selected scope and matching identities to coverage and final validation

#### Scenario: Fail an invalid plan

- **WHEN** Flow execution fails or its output is malformed
- **THEN** the selection job fails and cannot publish a successful validation record

### Requirement: Preserve post-execution validation ownership

Test execution, coverage evaluation, artifact publication, and final all-job success checks SHALL remain separate from the planning Flow. Only the completed successful attempt with matching source and coverage evidence SHALL advance the reusable baseline. Migrating selection SHALL NOT weaken the coverage gate or drop exact test filters.

#### Scenario: Reject failed downstream validation

- **WHEN** planning succeeds but a required test, coverage gate, or check fails
- **THEN** no reusable success record is published for that attempt

### Requirement: Express baseline policy through existing Flow nodes

The CI branch SHALL use Code and existing conditional/Loop/state capabilities to validate current identity, filter and order candidates by source ancestry, check record compatibility, and build incremental or cumulative comparison requests. Baseline policy SHALL NOT reside in a dedicated new node or an invoked Python selection script.

#### Scenario: Select the nearest usable success

- **WHEN** several compatible successful ancestor records exist
- **THEN** the Flow selects the nearest usable source ancestor with deterministic ties, independent of completion order

#### Scenario: Reject an invalid current execution

- **WHEN** current PR, workflow attempt, actual merge parents, or source identity is inconsistent
- **THEN** Flow execution fails without emitting a plan or treating the error as cumulative fallback

### Requirement: Acquire candidate evidence lazily

The Flow SHALL bound candidate search and acquire artifact bodies or missing Git objects only as needed for the current candidate. Acceptance SHALL stop further candidate I/O. Empty input, normal exhaustion, explicit cumulative mode, and discovery limits SHALL have explicit dispositions. Loop termination alone SHALL NOT imply successful baseline selection.

#### Scenario: Stop at a valid candidate

- **WHEN** the current candidate passes compatibility and Git identity checks
- **THEN** no subsequent candidate artifact read or Git fetch executes

#### Scenario: Handle no usable history

- **WHEN** the search has no usable record or cannot complete within its documented bounds
- **THEN** the Flow produces cumulative affected comparison with a reason, preserving shared-input deferrals

#### Scenario: Bypass search explicitly

- **WHEN** cumulative verification is requested or the candidate list is empty
- **THEN** the Flow does not enter an invalid candidate access and produces the appropriate cumulative comparison

### Requirement: Preserve migration compatibility and execution boundaries

Selection-policy compatibility SHALL be explicit across migration. Changed selection or execution semantics SHALL not reuse incompatible prior records. CI SHALL use package-level plans until the coverage consumer supports exact test filters. Required source, coverage, and completed-attempt checks SHALL remain enforced by final validation.

#### Scenario: Start under an incompatible policy

- **WHEN** the Flow selector runs with settings or policy incompatible with prior successes
- **THEN** it resets to cumulative comparison rather than excluding changes using old evidence

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
