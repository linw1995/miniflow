## ADDED Requirements

### Requirement: Separate construction and execution failures

Construction APIs SHALL return construction error types retaining typed sources. Runtime execution error types MUST NOT contain node construction, graph construction, or compiler failures. Compiler orchestration APIs MAY expose separate preparation and execution variants. Generated preparation SHALL report construction failures with node or scope attribution without converting them into execution errors.

#### Scenario: Preserve an embedded configuration error

- **WHEN** embedded node configuration cannot be decoded
- **THEN** preparation returns a construction error retaining the decode error source

#### Scenario: Launch a prepared stream

- **WHEN** the runtime starts a successfully prepared stream
- **THEN** launch can report input, runtime configuration, or resource failures but cannot report a compiler failure
