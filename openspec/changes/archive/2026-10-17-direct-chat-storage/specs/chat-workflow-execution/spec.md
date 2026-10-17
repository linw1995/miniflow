## MODIFIED Requirements

### Requirement: Embed the application workflows

The package SHALL embed conversation and session title definitions as Rust modules using the existing workflow embedding facility. Built hosts SHALL execute them without workflow source files, generated source files, build tools, or spawning standalone workflow runners.

#### Scenario: Run without definition files

- **WHEN** a built host runs with workflow and generated source files removed
- **THEN** both embedded workflows remain executable with their configured runtime resources

#### Scenario: Rebuild a workflow change

- **WHEN** an embedded definition changes and the package is rebuilt without cleaning
- **THEN** the resulting host executes the updated definition

### Requirement: Finalize execution failures outside the success path

The host SHALL finalize begun work after provider failure, deadline expiry, or cancellation through shared storage handlers even when success-path nodes do not execute. Cleanup SHALL run independently of the cancelled request without creating a workflow or execution context. Cleanup failures MUST preserve the original error and identify unresolved work.

#### Scenario: Cancel a pending conversation

- **WHEN** cancellation occurs while the LLM node waits for a response
- **THEN** the host finalizes the turn and call as cancelled without committing a reply
- **AND** subsequent submissions can proceed after cleanup succeeds

#### Scenario: Fail cleanup

- **WHEN** a provider failure is followed by a database failure during finalization
- **THEN** the host returns both failure contexts and the affected identities rather than reporting successful cleanup
