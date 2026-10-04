# Spec Delta

## MODIFIED Requirements

### Requirement: Resolve conditional stdin ownership before execution

Launchers SHALL derive stdin requirements from prepared metadata, data bindings, and validated startup
arguments. A file-bound source SHALL not require stdin. Multiple active stdin consumers or a missing required
stdin resource MUST fail before execution. TUI launch SHALL reject active stdin requirements and retain terminal keyboard ownership. File mode SHALL
launch through workflow parameters; standalone runners SHALL support stdin ingestion.

#### Scenario: Run two file sources

- **WHEN** both sources have supplied file paths
- **THEN** both execute independently without conflicting stdin declarations

#### Scenario: Reject two active stdin sources

- **WHEN** both sources omit path
- **THEN** launch rejects the exclusive-resource conflict before either source reads
