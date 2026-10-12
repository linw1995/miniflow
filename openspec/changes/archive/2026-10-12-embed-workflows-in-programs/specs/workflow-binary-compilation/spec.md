# Workflow Binary Compilation

## ADDED Requirements

### Requirement: Share code generation across distribution modes

Standalone compilation and host embedding SHALL reuse provider validation, immutable layout generation,
typed eligibility/fallback rules, and runtime preparation semantics. Generation SHALL accept artifact-location
and entry-point configuration without textual source replacement. Existing standalone API entry
points SHALL retain their default output paths, ordinal provider aliases, and runner behavior.

#### Scenario: Continue compiling a standalone runner

- **WHEN** an existing caller generates a standard workflow project without embedding configuration
- **THEN** the project retains its existing file paths and dependency aliases and compiles without new warnings

#### Scenario: Preserve execution parity across generation profiles

- **WHEN** the same workflow and compatible providers are compiled into a standalone runner and an embedded module
- **THEN** both produce equivalent selected outputs while embedding retains the existing custom-runner fallback
