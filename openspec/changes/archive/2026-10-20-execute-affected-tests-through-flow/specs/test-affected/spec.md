## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Compose a planning-only streaming path

The planning path SHALL connect `git.diff` as a Task to `lsp.analyze` as a Stream producer and then to `test_affected.plan` as a Task. This path SHALL finish planning before any optional execution branch launches the consumer. Original comparison information SHALL reach the plan node through the analysis output, without a bypass from the original message domain.

#### Scenario: Produce a plan for one job

- **WHEN** the initial Git task emits a comparison and finishes
- **THEN** analysis completes and the plan task produces one final plan before normal stream completion, without executing tests

#### Scenario: Reject a bypass edge

- **WHEN** a plan task consumes both the original Git output and the analysis producer output
- **THEN** existing cross-domain validation rejects the graph

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
