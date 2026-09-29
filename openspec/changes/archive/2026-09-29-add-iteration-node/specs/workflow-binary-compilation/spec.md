# Spec Delta

## ADDED Requirements

### Requirement: Validate and generate iteration bodies

The compiler SHALL plan and validate an Iteration body using the enclosing workflow's linked node packages. Validation MUST construct and check all body nodes and result ports even when the input array is empty. A generated runner SHALL prepare body nodes once and execute statically generated body calls per item inside a fresh child context. In-memory and generated execution MUST use the same input, output, skip, error, and type-checking semantics. The runner MUST remain standalone after installation.

#### Scenario: Match generated and in-memory execution

- **WHEN** the same Iteration workflow runs in memory and as an installed binary
- **THEN** both return the same ordered result array or an equivalent indexed error

#### Scenario: Reject an invalid body before installation

- **WHEN** a body node has invalid configuration or a missing required input
- **THEN** validation rejects the new runner and preserves an existing installed executable
