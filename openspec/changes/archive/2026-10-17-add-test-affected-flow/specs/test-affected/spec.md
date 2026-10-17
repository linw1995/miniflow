# Spec Delta

## Purpose

Produce an explainable affected-test plan from commit changes through a three-node Flow, with test execution delegated to a separate script.

## ADDED Requirements

### Requirement: Compose a planning-only streaming path

The Flow SHALL connect `git.diff` as a Task to `lsp.analyze` as a Stream producer and then to `test_affected.plan` as a Task. It SHALL NOT contain a test-execution node or launch an execution script. Original comparison information SHALL reach the plan node through the analysis output, without a bypass from the original message domain.

#### Scenario: Produce a plan for one job

- **WHEN** the initial Git task emits a comparison and finishes
- **THEN** analysis completes and the plan task produces one final plan before normal stream completion, without executing tests

#### Scenario: Reject a bypass edge

- **WHEN** a plan task consumes both the original Git output and the analysis producer output
- **THEN** existing cross-domain validation rejects the graph

### Requirement: Keep Git output factual

The Git node SHALL resolve local base/head revisions, their merge base, old/new file paths and ranges, and clean snapshot locations. It SHALL NOT compute test-selection policy or launch LSP. Missing revisions and dirty or mismatched snapshots SHALL fail without fetching history or switching the caller's checkout.

#### Scenario: Compare a diverged branch

- **WHEN** base contains commits absent from head
- **THEN** the change set excludes base-only changes through merge-base comparison

#### Scenario: Change dependency configuration

- **WHEN** a Cargo manifest changes
- **THEN** the Git node reports the file change without deciding which tests must run

### Requirement: Preserve snapshot lifetime for plan consumers

Snapshots SHALL remain available through analysis, planning, and later script consumption. Locations SHALL have explicit ownership and content identity. Returning from a node or finishing the Flow SHALL NOT invalidate them. Cleanup SHALL be limited to tool-owned paths and SHALL NOT change the caller's checkout.

#### Scenario: Consume a plan later

- **WHEN** a script reads a completed plan after the Flow exits
- **THEN** its snapshots remain available, or the script reports their removal rather than using unrelated sources

### Requirement: Build a conservative baseline in the planner

The plan task SHALL derive Cargo package ownership and reverse dependencies with all declared local dependency kinds retained. Successful baseline results SHALL agree with the current selector. Dependency configuration, shared or unknown inputs, and unusable dependency analysis SHALL retain full-workspace fallback; missing workspace metadata SHALL fail planning.

#### Scenario: Change shared nextest configuration

- **WHEN** the comparison changes `.config/nextest.toml`
- **THEN** the plan selects the full workspace and records that reason, independently of semantic results

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

The plan SHALL identify source/configuration snapshots, retained package or binary scopes, exact test selectors where justified, baseline fallback, and reasons. A supplied matching head inventory SHALL allow concrete case enumeration; otherwise unenumerated scopes SHALL be explicit. The Flow SHALL NOT compile or execute tests to obtain counts.

#### Scenario: Plan without compiled tests

- **WHEN** no matching executable inventory is supplied
- **THEN** the output retains required scopes/selectors and marks unavailable counts without building or running tests

### Requirement: Execute through a separate script

The plan-consumer script SHALL validate plan and snapshot/configuration identities, discover executable cases when needed, and invoke nextest outside the Flow. It SHALL reuse the discovery build. Missing or ambiguous cases SHALL widen to the recorded baseline with a diagnostic; stale identities or malformed plans SHALL fail before test execution.

#### Scenario: Validate generated test names

- **WHEN** source-derived selectors do not uniquely match the head inventory
- **THEN** the script reports the mismatch and retains the applicable baseline rather than silently dropping tests

#### Scenario: Reject changed sources

- **WHEN** a referenced snapshot has changed since planning
- **THEN** the script rejects the plan before executing tests

### Requirement: Preserve empty plans and execution failures

An empty plan SHALL cause the script to run no test command. Actual compilation or test failure SHALL retain diagnostics and an unsuccessful exit status, not become analysis fallback or success. Script cancellation SHALL stop further execution. The Flow SHALL remain uninvolved in executing or retrying these tests.

#### Scenario: Execute an empty plan

- **WHEN** the script consumes an empty selection
- **THEN** it reports no tests required without an unfiltered nextest invocation

#### Scenario: A selected test fails

- **WHEN** nextest reports a failed selected test
- **THEN** the script exits unsuccessfully without retrying a broader suite as selection recovery

### Requirement: Report cost without promising early policy gating

Reports SHALL separate Flow analysis/planning time from script inventory/build and test execution time. Selection comparisons SHALL use the same head/configuration. The three-node Flow SHALL NOT be described as avoiding LSP work for every eventual full fallback; final policy decisions occur after analysis. Reduced case counts SHALL NOT be claimed as measured time savings.

#### Scenario: Discover full fallback after analysis

- **WHEN** the planner finds shared-input changes after LSP has already analyzed source edits
- **THEN** the report retains that analysis cost and the full-workspace plan

### Requirement: Keep rollout opt-in

The planning Flow and consumer script SHALL NOT replace the current CI selector, change coverage flags, or weaken scheduled full verification in the initial delivery. Their own tests SHALL participate in development validation independently of future CI adoption.

#### Scenario: Add the tool to the workspace

- **WHEN** the tool packages become workspace members
- **THEN** current CI selection remains authoritative and developers can inspect or execute tool plans separately
