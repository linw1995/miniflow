# Spec Delta

## MODIFIED Requirements

### Requirement: Declare execution resources during preparation

Prepared metadata SHALL declare at most one stdin requirement per node: unconditional ownership or ownership
unless a named input is supplied. Data bindings and validated startup arguments SHALL resolve conditional
ownership before execution. Validation SHALL reject competing active owners without acquiring input.
Discovery MUST use provider metadata without built-in kind-name inference or a separate factory contract.

#### Scenario: Prepare an external stdin provider

- **WHEN** a third-party stream provider declares exclusive runtime stdin
- **THEN** its resource requirements are available for validation and launch without reading stdin

#### Scenario: Reject conflicting ownership

- **WHEN** two prepared nodes actively require exclusive use of the same input resource
- **THEN** validation reports both consumers and the resource before execution
