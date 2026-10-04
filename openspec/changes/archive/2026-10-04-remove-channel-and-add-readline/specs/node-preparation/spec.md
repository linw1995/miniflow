# Spec Delta

## MODIFIED Requirements

### Requirement: Declare execution resources during preparation

Prepared node metadata SHALL identify runtime input resources required for execution, including exclusive
stdin and stdin conditional on the absence of a named input. Data bindings and validated startup arguments
SHALL resolve conditional requirements before execution. Validation SHALL reject conflicting active owners
without acquiring those inputs. Resource discovery MUST use the selected provider's metadata without built-in kind-name inference or a
second metadata-only factory contract.

#### Scenario: Prepare an external stdin provider

- **WHEN** a third-party stream provider declares exclusive runtime stdin
- **THEN** its resource requirements are available for validation and launch without reading stdin

#### Scenario: Reject conflicting ownership

- **WHEN** two prepared nodes actively require exclusive use of the same input resource
- **THEN** validation reports both consumers and the resource before execution
