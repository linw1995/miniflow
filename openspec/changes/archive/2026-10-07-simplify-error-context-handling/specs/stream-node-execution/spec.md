## MODIFIED Requirements

### Requirement: Validate and attribute each emission

Emissions SHALL pass existing output and skip validation before publication. Invalid emissions SHALL fail the instance even if a plugin ignores a send error. Producer errors and panics SHALL identify the node, preserve delivered results, and suppress further publication. Panic failures SHALL retain the synthesized `StreamError` in their source chain. Observations SHALL report invocation identity, successful emission counts, and failure phase.

#### Scenario: Ignore an invalid send

- **WHEN** a producer sends the wrong output type and then returns success after ignoring the error
- **THEN** the instance still fails and the invalid result is never delivered

#### Scenario: Fail after a delivered prefix

- **WHEN** a producer fails after the consumer received earlier results
- **THEN** those results remain effective and the terminal failure identifies the producer

#### Scenario: Inspect a producer panic cause

- **WHEN** a producer panics during execution
- **THEN** the failure identifies that producer, retains the synthesized `StreamError` through the node error source, and includes the panic message in its diagnostic
