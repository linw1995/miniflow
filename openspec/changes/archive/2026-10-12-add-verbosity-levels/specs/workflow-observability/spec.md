## MODIFIED Requirements

### Requirement: Parse verbosity at runner option boundaries

Runner argument parsing SHALL preserve consumed option-like string values, schema validation, and transport-conflict
rules. `-v`, clusters such as `-vv`, and repeated `--verbose` SHALL accumulate with execution detail capped at three.
CLI counts SHALL accumulate before and after subcommands. CLI options SHALL precede trailing workflow parameters.

#### Scenario: Keep an option-like input value

- **WHEN** `--node.port -vvv` supplies a string value to a declared workflow input
- **THEN** that consumed value remains `-vvv` without increasing verbosity

#### Scenario: Accumulate verbosity

- **WHEN** flags are clustered, repeated, or placed on both sides of a CLI subcommand
- **THEN** their counts are added and any total above three selects level three

#### Scenario: Reject ambiguous option use

- **WHEN** malformed verbosity flags are supplied or named workflow parameters are mixed with JSON transport
- **THEN** parsing fails before node execution

### Requirement: Render readable local lifecycle summaries

At level one, local diagnostics SHALL retain readable progress, outcomes, timing, and port/failure context without
repeated routing fields. Readable Loop paths SHALL use one-based pass numbers and stream identifiers SHALL retain
unsigned precision. Summary lines SHALL show the run ID at workflow start. Level two SHALL add available event
name/sequence, workflow identity, node position, and node kind metadata.

#### Scenario: Scan successful execution

- **WHEN** a workflow executes with `-v`
- **THEN** its existing readable progress and completion summaries remain intact

#### Scenario: Locate a specific execution boundary

- **WHEN** a workflow executes with `-vv`
- **THEN** readable summaries identify the corresponding lifecycle event and available node/workflow metadata

#### Scenario: Correlate local output with export

- **WHEN** readable diagnostics and OTLP export are both enabled
- **THEN** the summary start line identifies the same run without repeating that ID on every readable event line

## ADDED Requirements

### Requirement: Display complete lifecycle records at the highest verbosity

Level three SHALL print the complete existing `WireRecord` as compact JSON on stderr, including its attributes, body,
time, and available trace context. It SHALL use the same record passed to OTel logging without changing export or
result stdout. All positive levels SHALL work without the OTLP SDK and retain readable 60-second node reminders.
Compilation SHALL enable its existing build summary at any positive level.

#### Scenario: Inspect a telemetry-disabled runner

- **WHEN** a runner compiled with `--no-telemetry` executes with `-vvv`
- **THEN** lifecycle JSON retains all available fields, result stdout remains intact, and active-node reminders continue

#### Scenario: Correlate the complete record with export

- **WHEN** a level-three runner has configured OTLP export
- **THEN** local records preserve the exported workflow/run identity and available trace correlation
