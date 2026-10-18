## MODIFIED Requirements

### Requirement: Attribute usage to reported model calls

The workflow SHALL explicitly return one usage record per reported model attempt, including repeated
calls and calls without counters. Records SHALL contain unique call identities, resolved public model
names, and node/loop identity when available. The backend MUST NOT infer a model from graph topology or
collect hidden node outputs. Admission SHALL NOT require configuration fingerprints.

#### Scenario: Combine multiple models

- **WHEN** a workflow concatenates two LLM usage lists
- **THEN** accounting retains both call identities, models, and reported counters

#### Scenario: Reject duplicated call facts

- **WHEN** one result contains repeated call IDs
- **THEN** result validation rejects it rather than charging the same call twice

#### Scenario: Override a model without recompilation

- **WHEN** a public model parameter overrides its default
- **THEN** accounting records the same resolved model forwarded to the runner

## ADDED Requirements

### Requirement: Recover without executable metadata

Decoding and reconciling captured output SHALL require only the admitted invocation and result bytes,
without rebuilding an executable manifest or conversation input contract. Existing bounded parsing,
ownership checks, and idempotent persistence SHALL remain in effect.

#### Scenario: Retry saving after the executable is unavailable

- **WHEN** the application retains result bytes and the admitted invocation
- **THEN** it can decode and save the result without loading the original executable or manifest
