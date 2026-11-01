# test-affected Specification

## Purpose

Produce explainable affected-test plans through one conditional local/CI Flow with shared Git comparison, optional LSP analysis, and conservative planning. Support explicit local execution through the existing consumer, while CI coverage evaluation and successful-validation publication remain separate.

## Requirements

### Requirement: Compose a planning-only streaming path

The planning path SHALL connect `git.diff` as a Task to `lsp.analyze` as a Stream producer and then to `test_affected.plan` as a Task. This path SHALL finish planning before any optional execution branch launches the consumer. Original comparison information SHALL reach the plan node through the analysis output, without a bypass from the original message domain.

#### Scenario: Produce a plan for one job

- **WHEN** the initial Git task emits a comparison and finishes
- **THEN** analysis completes and the plan task produces one final plan before normal stream completion, without executing tests

#### Scenario: Reject a bypass edge

- **WHEN** a plan task consumes both the original Git output and the analysis producer output
- **THEN** existing cross-domain validation rejects the graph

### Requirement: Keep Git output factual

The Git node SHALL resolve local revisions, old/new file paths and ranges, and clean snapshots. Default comparison SHALL use the merge base; explicit snapshot comparison SHALL compare the resolved supplied trees directly and report its mode. The node SHALL NOT decide CI baseline eligibility, test-selection policy, or launch LSP. Missing revisions and dirty or mismatched snapshots SHALL fail without fetching history or switching the caller's checkout.

#### Scenario: Compare a diverged branch

- **WHEN** default comparison uses a base containing commits absent from head
- **THEN** the change set excludes base-only changes through merge-base comparison

#### Scenario: Change dependency configuration

- **WHEN** a Cargo manifest changes
- **THEN** the Git node reports the file change without deciding which tests must run

#### Scenario: Compare successive tested snapshots

- **WHEN** explicit snapshot mode supplies two local merge snapshots whose shared parent is not the intended incremental base
- **THEN** the Git output retains the supplied resolved base and reports the direct tree diff rather than substituting their merge base

#### Scenario: Receive invalid snapshot inputs

- **WHEN** the requested comparison mode is unsupported or a required local revision cannot be resolved
- **THEN** the Git node fails without fetching history or silently choosing another comparison mode

### Requirement: Preserve snapshot lifetime for plan consumers

Snapshots SHALL remain available through analysis, planning, and later script consumption. Locations SHALL have explicit ownership and content identity. Returning from a node or finishing the Flow SHALL NOT invalidate them. Cleanup SHALL be limited to tool-owned paths and SHALL NOT change the caller's checkout.

#### Scenario: Consume a plan later

- **WHEN** a script reads a completed plan after the Flow exits
- **THEN** its snapshots remain available, or the script reports their removal rather than using unrelated sources

### Requirement: Build a conservative baseline in the planner

The plan task SHALL own conservative package selection and retain all declared local dependency kinds. Package-owned inputs, including manifests, SHALL select their owner and reverse dependencies. Known shared executables SHALL seed their consumers. Explicit repository configuration and documentation exceptions SHALL defer to full main/nightly runs. Unknown inputs and analyzer failures SHALL retain full fallback; missing metadata SHALL fail planning.

#### Scenario: Change shared nextest configuration

- **WHEN** only `.config/nextest.toml`, root `Cargo.toml`, or root `Cargo.lock` changes
- **THEN** the plan records the paths in `ignored_files` without selecting Rust tests

#### Scenario: Change a package manifest

- **WHEN** a package manifest or its nested fixture manifest changes
- **THEN** package ownership selects that package and its reverse dependencies without forcing the entire workspace

#### Scenario: Change a known shared executable input

- **WHEN** examples or generated-runner build helpers change
- **THEN** their known consumer packages and reverse dependencies remain selected

#### Scenario: Encounter an unclassified input

- **WHEN** a shared input has no rule or none of its expected consumers exist
- **THEN** the plan selects the full workspace and records the reason

#### Scenario: Preserve inactive dependency edges

- **WHEN** a local dependency is declared but inactive on the host target
- **THEN** it remains in conservative reverse-dependency selection

### Requirement: Use changes as the impact starting point

The planner SHALL combine original Git changes with base/head semantic evidence rather than relying only on differences between semantic graphs. Deleted, renamed, and unmatched definitions SHALL retain relevant base evidence or conservative scope. Final test identities SHALL refer to the head configuration.

#### Scenario: Change a function body without changing calls

- **WHEN** a function's behavior changes but its name and call edges remain identical
- **THEN** the original source diff still marks it as changed and its test impact is not discarded

#### Scenario: Delete a definition

- **WHEN** a definition exists only in base
- **THEN** its base relationships are considered or the corresponding conservative scope remains selected

### Requirement: Keep policy inside the planner

Classification, selection, union, fallback, and plan formatting SHALL NOT require separate nodes. The initial tool SHALL use concrete Cargo and nextest support without requiring a generic adapter registry. These ecosystem details SHALL remain outside the language-neutral LSP provider.

#### Scenario: Construct the final selection

- **WHEN** the plan task receives the comparison and semantic outcomes
- **THEN** it applies local policy and emits a plan without invoking another policy or reporting node

### Requirement: Refine verified isolated test changes

An edit SHALL narrow to exact tests only with verified exclusive test ownership, unchanged surrounding production/shared setup, and executable mapping. Production edits in the same comparison SHALL retain reverse-dependent scope. Shared helpers, reused tests, registrations, ambiguous macros, and missing subprocess relationships SHALL retain unproved scope.

#### Scenario: Combine test-only and production edits

- **WHEN** two runtime edits affect verified isolated test bodies and another package changes production code
- **THEN** the plan retains both tests and all tests in the production package and its consumers

#### Scenario: Cross a subprocess boundary

- **WHEN** a test invokes changed code in a subprocess without a verified semantic edge
- **THEN** absence of the edge does not remove that test from the baseline

### Requirement: Validate and interpret analysis outcomes conservatively

The planner SHALL verify outcome snapshot/configuration identities against forwarded context. Empty relationships SHALL NOT authorize exclusions. Missing optional capabilities SHALL retain unproved scope; server startup, deadline, or workspace-analysis failures SHALL retain full-workspace fallback. Cancelled or invalid analysis SHALL NOT produce a usable success plan.

#### Scenario: Receive a server timeout

- **WHEN** analysis reports an incomplete outcome caused by its deadline
- **THEN** the planner emits a full-workspace fallback plan with the timeout reason but runs no tests

#### Scenario: Detect a mismatched outcome

- **WHEN** evidence belongs to another snapshot or configuration
- **THEN** planning fails instead of emitting a stale or mixed plan

### Requirement: Emit a structured selection artifact

The plan SHALL identify source/configuration snapshots, comparison mode and resolved base/head, retained package or binary scopes, exact test selectors where justified, baseline fallback, reasons, and deferred `ignored_files`. A matching head inventory SHALL allow concrete case enumeration; otherwise unenumerated scopes SHALL be explicit. Planning SHALL NOT build or execute tests to populate plan counts. Saved plans SHALL remain consumable with defaults for absent comparison metadata.

#### Scenario: Plan without compiled tests

- **WHEN** no matching executable inventory is supplied
- **THEN** the output retains required scopes/selectors and marks unavailable counts without building or running tests

#### Scenario: Consume an existing saved plan

- **WHEN** a previously produced plan omits `ignored_files` or comparison mode
- **THEN** the consumer treats ignored files as empty and the comparison mode as the previous merge-base default while preserving its execution selection and source checks

#### Scenario: Preserve the explicit incremental comparison

- **WHEN** the planner receives analysis for an explicit snapshot comparison
- **THEN** the resulting plan retains that mode and exact comparison identities without resolving a new base

### Requirement: Execute through a separate script

The plan-consumer script SHALL validate plan and snapshot/configuration identities, discover executable cases when needed, and invoke nextest as a separate process, directly or through the Flow's explicit local execution branch. It SHALL reuse the discovery build. Missing or ambiguous cases SHALL widen to the recorded baseline with a diagnostic; stale identities or malformed plans SHALL fail before test execution.

#### Scenario: Validate generated test names

- **WHEN** source-derived selectors do not uniquely match the head inventory
- **THEN** the script reports the mismatch and retains the applicable baseline rather than silently dropping tests

#### Scenario: Reject changed sources

- **WHEN** a referenced snapshot has changed since planning
- **THEN** the script rejects the plan before executing tests

### Requirement: Preserve empty plans and execution failures

An empty plan SHALL cause the script to run no test command. Actual compilation or test failure SHALL retain diagnostics and an unsuccessful exit status, not become analysis fallback or success. Script cancellation SHALL stop further execution. The execution branch SHALL propagate failure without retrying or expanding scope after test failure.

#### Scenario: Execute an empty plan

- **WHEN** the script consumes an empty selection
- **THEN** it reports no tests required without an unfiltered nextest invocation

#### Scenario: A selected test fails

- **WHEN** nextest reports a failed selected test
- **THEN** the script exits unsuccessfully without retrying a broader suite as selection recovery

### Requirement: Report cost without promising early policy gating

Reports SHALL separate Flow analysis/planning time from script inventory/build and test execution time. Selection comparisons SHALL use the same head/configuration. The shared semantic analysis path SHALL NOT be described as avoiding LSP work for every eventual full fallback; final policy decisions occur after analysis. Reduced case counts SHALL NOT be claimed as measured time savings.

#### Scenario: Discover full fallback after analysis

- **WHEN** the planner finds unclassified shared-input changes after LSP has already analyzed source edits
- **THEN** the report retains that analysis cost and the full-workspace plan

### Requirement: Execute one conditional planning Flow

One Flow definition and compiled executable SHALL serve local and CI planning through existing `mf` commands. Parameters and if-else SHALL route to local request construction or CI policy composition using Code and Loop. Both branches SHALL converge at one Git request input through proven exclusive bindings, followed by shared LSP and planning. A successful admitted invocation SHALL emit exactly one explicit plan; invalid invocation SHALL fail.

#### Scenario: Default to local planning

- **WHEN** a caller omits `ci`, including when `GITHUB_ACTIONS` is set
- **THEN** `ci` defaults to false and the local branch runs without activating CI resolution; selecting CI requires explicit `ci=true`

#### Scenario: Plan from a CI success

- **WHEN** the CI branch obtains a compatible prior success
- **THEN** its comparison and provenance survive the analyzer message boundary into the planning result

### Requirement: Reuse proven exclusive input bindings

The Flow SHALL use version `2026-10-19` input references and the existing exclusive-input contract without adding a merge node or runtime selection policy. Both request producers SHALL depend on distinct outputs of the same if-else router and share the Git consumer's input domain. Correlated comparison fields SHALL travel together in one request value.

#### Scenario: Prove branch convergence

- **WHEN** local and CI request outputs bind the Git request input with an exclusive input declaration under the same router
- **THEN** compilation proves their mutual exclusion and the consumer receives the selected request once

#### Scenario: Reject unsafe rewiring

- **WHEN** a candidate is detached from the shared router or crosses a producer boundary
- **THEN** existing compilation rules reject the Flow before either branch executes

### Requirement: Snapshot local comparison inputs

The Git provider SHALL support worktree, first-parent commit, merge-base, and direct-snapshot comparisons. Worktree comparison SHALL include staged, unstaged, and untracked nonignored inputs in an immutable runnable snapshot without altering the caller's files, index, or branch. Root commits SHALL have a defined empty comparison baseline. Existing explicit-ref callers SHALL remain supported.

#### Scenario: Capture a dirty workspace

- **WHEN** tracked and untracked local inputs differ from HEAD
- **THEN** the plan identifies a snapshot containing those inputs while the original worktree and index remain unchanged

#### Scenario: Compare a root commit

- **WHEN** a selected commit has no parent
- **THEN** its contents are compared against an empty baseline

### Requirement: Keep planning precision inside existing providers

Planning precision SHALL be an option of the existing Git/analyzer/planner path, not a separate node kind. Package-only requests SHALL retain conservative package scope and may pass an empty analysis batch through the analyzer's existing not-requested outcome. Semantic requests SHALL preserve current evidence-gap and failure behavior.

#### Scenario: Request package-only planning

- **WHEN** CI requests package scope
- **THEN** the result contains complete selected packages and no exact-case filter that a package-only coverage consumer could discard

### Requirement: Execute local selections explicitly

The shared Flow SHALL accept a default-false `run` parameter. Enabled local execution SHALL invoke the existing snapshot-bound test consumer through `process.run` after planning. Execution configuration SHALL cross the analyzer boundary through opaque context, retaining existing scope checks. Plan-only output SHALL remain compatible; execution SHALL add an optional structured report. CI SHALL reject enabled execution and retain its coverage pipeline.

#### Scenario: Keep planning as the default

- **WHEN** a caller omits run or sets it false
- **THEN** no execution consumer is started and the Flow emits the existing plan and validation outputs

#### Scenario: Execute a local plan

- **WHEN** a local caller enables run and planning succeeds
- **THEN** the consumer validates and executes the selected snapshot scope, and Flow success includes its report

#### Scenario: Preserve empty and failed selections

- **WHEN** selection is empty or the selected tests fail
- **THEN** empty selection succeeds without invoking tests, while test failure makes the Flow fail

#### Scenario: Preserve CI coverage ownership

- **WHEN** ci and run are both true
- **THEN** the Flow fails before external CI acquisition or test execution

### Requirement: Accept consumer plans from stdin without mixing reports

The existing test consumer SHALL accept a plan path or stdin while preserving the same validation and execution behavior. Stdout SHALL contain one structured success report; child diagnostics SHALL use stderr. Compilation, testing, and cancellation errors SHALL retain failing process exit status.

#### Scenario: Consume a Flow plan directly

- **WHEN** the execution Task sends a complete plan on stdin
- **THEN** the consumer applies the same snapshot checks, inventory checks, exact filters, and fallback rules as a path-based invocation

### Requirement: Narrow package scope through a coverage graph

When a compatible coverage graph resolves for the comparison base, the plan SHALL map changed line ranges to exact test selectors before retaining package scope. Ranges the graph reports as unresolved SHALL widen their files to the existing package and reverse-dependency scope, with reasons recorded per scope.

#### Scenario: Select exact tests from the graph

- **WHEN** a production-code change maps to covering tests in a compatible graph
- **THEN** the plan selects exactly those tests for the owning package instead of the whole package, recording the graph commit as provenance

#### Scenario: Widen unresolved changes

- **WHEN** part of the change is unresolved by the graph
- **THEN** the affected files widen to package and reverse-dependency scope while resolved files retain exact selectors, with reasons recorded per scope

#### Scenario: Keep new test functions executable during graph narrowing

- **WHEN** a changed file contains an added or modified recognized test function and also changes lines the graph can narrow
- **THEN** the plan includes that test function in the exact selection alongside graph-derived selectors, because the graph indexes only base-side lines and cannot attribute new test bodies

#### Scenario: Widen unproven test identities

- **WHEN** a changed file contains test edits whose exact identities cannot be proven
- **THEN** the file widens to package scope so the changed tests still execute, rather than relying on base-side graph evidence

### Requirement: Preserve existing selection tiers and fallbacks

Graph unavailability SHALL retain package-level selection with a recorded reason and SHALL NOT itself force full-workspace fallback. Test-body isolation, shared-input policy, unknown-input fallback, analyzer-failure fallback, and empty-selection semantics SHALL remain unchanged and take precedence as today.

#### Scenario: Plan without a graph

- **WHEN** no compatible graph is available
- **THEN** the plan falls back to existing package-level selection, records the reason, and preserves empty-selection semantics

#### Scenario: Compose with test-body isolation

- **WHEN** a change qualifies for test-body isolation and a graph is also available
- **THEN** the existing isolation result applies and the graph is not consulted for that file
