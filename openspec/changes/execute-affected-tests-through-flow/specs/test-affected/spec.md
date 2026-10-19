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
