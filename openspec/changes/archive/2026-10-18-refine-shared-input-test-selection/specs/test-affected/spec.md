## MODIFIED Requirements

### Requirement: Build a conservative baseline in the planner

The plan task SHALL retain all declared local dependency kinds and agree with the current selector. Package-owned inputs, including manifests, SHALL select their owner and reverse dependencies. Known shared executables SHALL seed their consumers. Explicit repository configuration and documentation exceptions SHALL defer to full main/nightly runs. Unknown inputs and analyzer failures SHALL retain full fallback; missing metadata SHALL fail planning.

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

### Requirement: Emit a structured selection artifact

The plan SHALL identify source/configuration snapshots, retained package or binary scopes, exact test selectors where justified, baseline fallback, reasons, and `ignored_files` for deferred inputs. A supplied matching head inventory SHALL allow concrete case enumeration; otherwise unenumerated scopes SHALL be explicit. The Flow SHALL NOT compile or execute tests to obtain counts.

#### Scenario: Plan without compiled tests

- **WHEN** no matching executable inventory is supplied
- **THEN** the output retains required scopes/selectors and marks unavailable counts without building or running tests

#### Scenario: Consume an existing saved plan

- **WHEN** a previously produced plan omits `ignored_files`
- **THEN** the consumer treats the field as empty and preserves its execution selection

### Requirement: Report cost without promising early policy gating

Reports SHALL separate Flow analysis/planning time from script inventory/build and test execution time. Selection comparisons SHALL use the same head/configuration. The three-node Flow SHALL NOT be described as avoiding LSP work for every eventual full fallback; final policy decisions occur after analysis. Reduced case counts SHALL NOT be claimed as measured time savings.

#### Scenario: Discover full fallback after analysis

- **WHEN** the planner finds unclassified shared-input changes after LSP has already analyzed source edits
- **THEN** the report retains that analysis cost and the full-workspace plan
