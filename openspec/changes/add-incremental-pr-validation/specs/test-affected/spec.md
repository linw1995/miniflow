## MODIFIED Requirements

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

### Requirement: Emit a structured selection artifact

The plan SHALL identify source/configuration snapshots, comparison mode and resolved base/head, retained package or binary scopes, exact test selectors where justified, baseline fallback, reasons, and deferred `ignored_files`. A matching head inventory SHALL allow concrete case enumeration; otherwise unenumerated scopes SHALL be explicit. The Flow SHALL NOT compile or execute tests to obtain counts. Saved plans SHALL remain consumable with defaults for absent comparison metadata.

#### Scenario: Plan without compiled tests

- **WHEN** no matching executable inventory is supplied
- **THEN** the output retains required scopes/selectors and marks unavailable counts without building or running tests

#### Scenario: Consume an existing saved plan

- **WHEN** a previously produced plan omits `ignored_files` or comparison mode
- **THEN** the consumer treats ignored files as empty and the comparison mode as the previous merge-base default while preserving its execution selection and source checks

#### Scenario: Preserve the explicit incremental comparison

- **WHEN** the planner receives analysis for an explicit snapshot comparison
- **THEN** the resulting plan retains that mode and exact comparison identities without resolving a new base
