## ADDED Requirements

### Requirement: Compose a Task-only planning path

The planning path SHALL connect `git.diff` as a Task directly to `test_affected.plan` as a Task, with the coverage-graph outcome merged into the comparison context beforehand. This path SHALL finish planning before any optional execution branch launches the consumer.

#### Scenario: Produce a plan for one job

- **WHEN** the Git task emits a comparison and finishes
- **THEN** the plan task produces one final plan before normal completion, without executing tests

### Requirement: Report planning and execution cost

Reports SHALL separate Flow planning time from script inventory/build and test execution time. Selection comparisons SHALL use the same head/configuration. Reduced case counts SHALL NOT be claimed as measured time savings.

#### Scenario: Discover full fallback during planning

- **WHEN** the planner finds unclassified shared-input changes
- **THEN** the report retains that planning cost and the full-workspace plan

### Requirement: Preserve selection tiers and fallbacks

Graph unavailability SHALL retain package-level selection with a recorded reason and SHALL NOT itself force full-workspace fallback. Shared-input policy, unknown-input fallback, and empty-selection semantics SHALL remain unchanged and take precedence over graph narrowing.

#### Scenario: Plan without a graph

- **WHEN** no compatible graph is available
- **THEN** the plan falls back to existing package-level selection, records the reason, and preserves empty-selection semantics

## MODIFIED Requirements

### Requirement: Keep Git output factual

The Git node SHALL resolve local revisions, changed file paths with base-side line intervals, and clean snapshots. Default comparison SHALL use the merge base; explicit snapshot comparison SHALL compare the resolved supplied trees directly and report its mode. The node SHALL NOT decide CI baseline eligibility or test-selection policy. Missing revisions and dirty or mismatched snapshots SHALL fail without fetching history or switching the caller's checkout.

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

### Requirement: Build a conservative baseline in the planner

The plan task SHALL own conservative package selection and retain all declared local dependency kinds. Package-owned inputs, including manifests, SHALL select their owner and reverse dependencies. Known shared executables SHALL seed their consumers. Explicit repository configuration and documentation exceptions SHALL defer to full main/nightly runs. Unknown inputs SHALL retain full fallback; missing metadata SHALL fail planning.

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

### Requirement: Keep policy inside the planner

Classification, selection, union, fallback, and plan formatting SHALL NOT require separate nodes. The initial tool SHALL use concrete Cargo and nextest support without requiring a generic adapter registry.

#### Scenario: Construct the final selection

- **WHEN** the plan task receives the comparison
- **THEN** it applies local policy and emits a plan without invoking another policy or reporting node

### Requirement: Execute one conditional planning Flow

One Flow definition and compiled executable SHALL serve local and CI planning through existing `mf` commands. Parameters and if-else SHALL route to local request construction or CI policy composition using Code and Loop. Both branches SHALL converge at one Git request input through proven exclusive bindings, followed by shared planning. A successful admitted invocation SHALL emit exactly one explicit plan; invalid invocation SHALL fail.

#### Scenario: Default to local planning

- **WHEN** a caller omits `ci`, including when `GITHUB_ACTIONS` is set
- **THEN** `ci` defaults to false and the local branch runs without activating CI resolution; selecting CI requires explicit `ci=true`

#### Scenario: Plan from a CI success

- **WHEN** the CI branch obtains a compatible prior success
- **THEN** its comparison and provenance survive into the planning result

### Requirement: Keep planning precision inside existing providers

Planning precision SHALL be an option of the existing Git/planner path, not a separate node kind. Package-only requests SHALL retain conservative package scope. Test-precision requests MAY narrow through a compatible coverage graph and SHALL preserve its degradation behavior.

#### Scenario: Request package-only planning

- **WHEN** CI requests package scope
- **THEN** the result contains complete selected packages and no exact-case filter that a package-only coverage consumer could discard

### Requirement: Execute local selections explicitly

The shared Flow SHALL accept a default-false `run` parameter. Enabled local execution SHALL invoke the existing snapshot-bound test consumer through `process.run` after planning. Execution configuration SHALL cross into planning through opaque context, retaining existing scope checks. Plan-only output SHALL remain compatible; execution SHALL add an optional structured report. CI SHALL reject enabled execution and retain its coverage pipeline.

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

## REMOVED Requirements

### Requirement: Compose a planning-only streaming path

**Reason**: The planning path is Task-only; the language-server Stream producer is removed.

### Requirement: Use changes as the impact starting point

**Reason**: Semantic evidence from language analysis is removed with the analyzer; Git changes remain the only impact seed.

### Requirement: Refine verified isolated test changes

**Reason**: Test-body isolation is removed; coverage-graph narrowing is the only exact-selection mechanism.

### Requirement: Validate and interpret analysis outcomes conservatively

**Reason**: There is no analysis outcome to validate; the planner consumes the Git comparison directly.

### Requirement: Report cost without promising early policy gating

**Reason**: Replaced by the Task-only planning cost report without analysis stages.

### Requirement: Preserve existing selection tiers and fallbacks

**Reason**: Replaced by selection tiers without test-body isolation and analyzer-failure fallback.
