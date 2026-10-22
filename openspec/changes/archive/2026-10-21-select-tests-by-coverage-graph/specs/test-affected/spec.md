## ADDED Requirements

### Requirement: Narrow package scope through a coverage graph

When a compatible coverage graph resolves for the comparison base, the plan SHALL map changed line ranges to exact test selectors before retaining package scope. Ranges the graph reports as unresolved SHALL widen their files to the existing package and reverse-dependency scope, with reasons recorded per scope.

#### Scenario: Select exact tests from the graph

- **WHEN** a production-code change maps to covering tests in a compatible graph
- **THEN** the plan selects exactly those tests for the owning package instead of the whole package, recording the graph commit as provenance

#### Scenario: Widen unresolved changes

- **WHEN** part of the change is unresolved by the graph
- **THEN** the affected files widen to package and reverse-dependency scope while resolved files retain exact selectors, with reasons recorded per scope

### Requirement: Preserve existing selection tiers and fallbacks

Graph unavailability SHALL retain package-level selection with a recorded reason and SHALL NOT itself force full-workspace fallback. Test-body isolation, shared-input policy, unknown-input fallback, analyzer-failure fallback, and empty-selection semantics SHALL remain unchanged and take precedence as today.

#### Scenario: Plan without a graph

- **WHEN** no compatible graph is available
- **THEN** the plan falls back to existing package-level selection, records the reason, and preserves empty-selection semantics

#### Scenario: Compose with test-body isolation

- **WHEN** a change qualifies for test-body isolation and a graph is also available
- **THEN** the existing isolation result applies and the graph is not consulted for that file
