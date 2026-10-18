## ADDED Requirements

### Requirement: Execute one conditional planning Flow

One Flow definition and compiled executable SHALL serve local and CI planning through existing `mf` commands. Parameters and if-else SHALL route to local request construction or CI policy composition using Code and Loop. Both branches SHALL converge at one Git request input through proven exclusive edges, followed by shared LSP and planning. A successful admitted invocation SHALL emit exactly one explicit plan; invalid invocation SHALL fail.

#### Scenario: Plan locally without CI setup

- **WHEN** a caller selects local planning and supplies no CI identity or credentials
- **THEN** the local branch completes through the shared analysis path without activating CI resolution

#### Scenario: Plan from a CI success

- **WHEN** the CI branch obtains a compatible prior success
- **THEN** its comparison and provenance survive the analyzer message boundary into the planning result

### Requirement: Reuse proven exclusive input bindings

The Flow SHALL use the existing exclusive-input contract without adding a merge node or runtime selection policy. Both request producers SHALL depend on distinct outputs of the same if-else router and share the Git consumer's input domain. Correlated comparison fields SHALL travel together in one request value.

#### Scenario: Prove branch convergence

- **WHEN** local and CI request outputs bind the Git request input with exclusive edges under the same router
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

## MODIFIED Requirements

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

### Requirement: Report cost without promising early policy gating

Reports SHALL separate Flow analysis/planning time from script inventory/build and test execution time. Selection comparisons SHALL use the same head/configuration. The shared semantic analysis path SHALL NOT be described as avoiding LSP work for every eventual full fallback; final policy decisions occur after analysis. Reduced case counts SHALL NOT be claimed as measured time savings.

#### Scenario: Discover full fallback after analysis

- **WHEN** the planner finds unclassified shared-input changes after LSP has already analyzed source edits
- **THEN** the report retains that analysis cost and the full-workspace plan

## REMOVED Requirements

### Requirement: Keep rollout opt-in

**Reason**: This change makes the shared Flow authoritative for CI test planning after compatibility validation.

**Migration**: Replace Python selection with the compiled Flow. Keep execution and final validation in existing CI stages and retain full main/nightly suites. Local callers use the same Flow's local branch.
