## ADDED Requirements

### Requirement: Emit local verbose lifecycle diagnostics

Generated runners and `mf run` SHALL accept `-v` and `--verbose` to print existing lifecycle metadata on stderr.
Diagnostics SHALL preserve result stdout and workflow outcomes without automatically dumping node configuration or
input/output records. They SHALL include available identities, outcomes, timing, Loop paths, and streaming invocation
metadata.

#### Scenario: Observe finite and streaming execution

- **WHEN** a task, Loop, or streaming workflow executes verbosely
- **THEN** its existing lifecycle boundaries are printed without changing the selected result protocol

### Requirement: Keep local diagnostics independent of export

Local diagnostics SHALL work without exporter configuration and with `--no-telemetry`, using no global provider
installation or export connection. Configured OTLP export SHALL coexist with local diagnostics and retain its existing
buffering and shutdown behavior.

#### Scenario: Diagnose a standalone workflow without a collector

- **WHEN** a standalone runner executes with `-v` and no OTLP endpoint
- **THEN** lifecycle metadata appears on stderr while result stdout and workflow exit behavior remain intact

#### Scenario: Diagnose a telemetry-disabled runner

- **WHEN** a runner compiled with `--no-telemetry` executes with `--verbose`
- **THEN** it prints local lifecycle metadata without requiring the OTLP SDK or opening an export connection

#### Scenario: Combine local diagnostics with export

- **WHEN** a verbose runner has a configured OTLP endpoint and completes successfully or with a handled failure
- **THEN** local diagnostics coexist with correlated export and preserve the workflow outcome

### Requirement: Parse verbosity at runner option boundaries

Runner argument parsing SHALL preserve consumed option-like string values and retain existing schema validation and
transport-conflict rules. Repeated verbose options SHALL be rejected. CLI options SHALL precede trailing named
workflow parameters.

#### Scenario: Keep an option-like input value

- **WHEN** `--node.port -v` supplies a string value to a declared workflow input
- **THEN** that consumed value remains `-v` rather than becoming a verbose option

#### Scenario: Reject ambiguous option use

- **WHEN** verbose options repeat or named parameters are mixed with JSON transport
- **THEN** argument parsing fails before node execution
