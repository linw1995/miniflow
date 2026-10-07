## MODIFIED Requirements

### Requirement: Return complete prepared nodes

Node factories SHALL return a prepared node containing configuration-derived metadata and its execution
implementation. Metadata SHALL include ports, output derivations, and declared context references.
The compiler SHALL validate and resolve this metadata before execution. Execution traits MUST NOT
require metadata hooks or methods that complete a partially constructed node. Configured input declarations and
resource conditions MUST remain stable between generated-build and runtime preparation for the same configuration
and selected provider, including across host and target implementations. Runtime resource initialization MAY fail
without changing these declarations.

#### Scenario: Prepare dynamic ports

- **WHEN** a provider derives ports from its configuration
- **THEN** its factory returns those ports as metadata and compilation validates them without executing the task

#### Scenario: Preserve configured declarations across environments

- **WHEN** the same configured provider is prepared during generated compilation and runner startup in different process environments
- **THEN** it reports the same input declarations and resource conditions while retaining independent executor state

#### Scenario: Preserve declarations across host and target builds

- **WHEN** host and target builds of a selected provider use platform-specific executor initialization
- **THEN** their configured input declarations and resource conditions agree

#### Scenario: Keep initialization failures separate

- **WHEN** a provider cannot initialize its executor because a required runtime resource is unavailable
- **THEN** preparation reports the construction failure without substituting a different input contract
