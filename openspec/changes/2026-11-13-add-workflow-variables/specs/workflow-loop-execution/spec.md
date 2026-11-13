## ADDED Requirements

### Requirement: Accept generic assignment in Loop bodies

For definition version `2026-11-13`, `workflow.assign` SHALL support the same typed assignment and
`done` publication contract as `workflow.loop_assign` in a Loop body. It SHALL target only the
nearest frame's writable variables, not read-only inputs or caller root variables. Existing
`workflow.loop_assign` and `workflow.exit_loop` syntax SHALL remain Loop-only.

#### Scenario: Refine a Loop variable with generic assignment

- **WHEN** a Loop body uses `workflow.assign` to overwrite one of its variables
- **THEN** subsequent dependent reads and the next pass see the new value
- **AND** the final Loop output contains the last committed value

#### Scenario: Reject a root-only target in a Loop

- **WHEN** a Loop assignment names a variable declared only in the enclosing Flow's root state
- **THEN** preparation rejects the unknown current-frame variable

#### Scenario: Reject overlap with an end-of-pass update

- **WHEN** `workflow.assign` and a body output selection target the same Loop variable
- **THEN** preparation rejects the conflicting immediate and end-of-pass writes

#### Scenario: Combine distinct update targets

- **WHEN** a generic assignment changes one variable and a body output selects a different variable
- **THEN** dependent body reads observe the immediate write before the complete pass commits its output update
- **AND** the enclosing Flow's root variables remain unchanged

#### Scenario: Exit before an end-of-pass producer

- **WHEN** a generic assignment completes and an explicit exit leaves a selected output producer unvisited
- **THEN** the Loop preserves the immediate write and abandons the end-of-pass update without reading the unvisited output
- **AND** interpreted and generated runners retain the same root and Loop state
