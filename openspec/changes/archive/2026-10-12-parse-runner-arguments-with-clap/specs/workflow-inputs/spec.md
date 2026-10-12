## ADDED Requirements

### Requirement: Describe runner arguments without execution

New compiled runners SHALL accept `--help` and `-h`, write schema-specific help to stdout, and exit with
status 0. Help SHALL list qualified inputs, types, required flags, JSON transports, and verbosity. With
`--node ID`, it SHALL show only the selected task's inputs. Help MUST NOT require input values, read input
or context files, construct providers, initialize telemetry, or execute nodes.

#### Scenario: Inspect missing required inputs

- **WHEN** a runner receives `--help` without its required startup inputs
- **THEN** it lists input contracts and exits successfully without preparing or executing nodes

#### Scenario: Inspect a selected task without context files

- **WHEN** a runner receives `--node transform --context-file missing.json --help`
- **THEN** help describes only that task's inputs and does not open the context file

### Requirement: Parse runner CLI syntax consistently

CLI syntax errors SHALL write diagnostics to stderr and exit with status 2. Domain validation failures
SHALL retain typed diagnostics and a nonzero status. Input transports SHALL accept separate and inline
values while rejecting duplicates and mixing with named inputs. Short-option-looking named values SHALL
remain literal; values starting with `--` SHALL require inline syntax. Clap's standard end-of-options
marker SHALL be accepted, followed by ordinary schema validation.

#### Scenario: Reject unknown runner syntax

- **WHEN** a runner receives an unknown CLI option
- **THEN** it writes a CLI diagnostic to stderr and exits with status 2 before business execution

#### Scenario: Reject individually valid mixed transports

- **WHEN** valid named inputs are combined with a valid JSON input document
- **THEN** the runner rejects the combination instead of selecting one transport

#### Scenario: Preserve option-looking literal values

- **WHEN** a string parameter receives `-v` as its following value or `--verbose` as an inline value
- **THEN** the supplied value remains literal input data rather than a verbosity option
