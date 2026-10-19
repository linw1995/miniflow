# process-execution Specification

## Purpose

Provide bounded Unix process execution for repository tooling, with structured input/output, explicit failures, and owned process cleanup.

## Requirements

### Requirement: Execute a configured program with structured transport

The tooling package SHALL provide `process.run` as an ordinary Task with configured program and argument vector, optional working directory and JSON stdin, and bounded stdout. It SHALL support text output and explicit single-value JSON decoding. It SHALL not invoke a shell implicitly or implement test-selection policy. JSON input sensitivity SHALL propagate to the result.

#### Scenario: Exchange a JSON document

- **WHEN** the Task invokes a program with JSON stdin and JSON output mode
- **THEN** the program receives the serialized input and one successfully decoded result is published independently of stderr diagnostics

### Requirement: Preserve process failure and ownership

Nonzero exit, invalid required output, timeout, and cancellation SHALL fail execution without a successful result. Unix process work SHALL use an owned process group, bounded termination grace, and forced cleanup/reaping where necessary. Input/output size and elapsed-time limits SHALL be explicit. Diagnostics SHALL not automatically include supplied argument or input contents.

#### Scenario: Cancel a running process

- **WHEN** cancellation arrives while the process or its same-group descendants are running
- **THEN** the group is terminated and reaped, including descendants ignoring graceful termination

#### Scenario: Reject an unsuccessful report

- **WHEN** the program fails, exceeds stdout bounds, or returns malformed JSON in JSON mode
- **THEN** the Task fails and publishes no successful report
