## ADDED Requirements

### Requirement: Share runtime snapshot notifications across execution modes

Finite and streaming execution SHALL use the same opt-in SnapshotRecorder callback
and ordinary-task capture boundary. The runtime recorder SHALL only notify current
node snapshots and existing execution identities. Caller-owned recording SHALL
manage dictionaries, encoding, history, and diagnostics. The runtime MUST NOT keep
a separate stream data observer or duplicate ordinary-task data capture path.

#### Scenario: Observe a finite task through the common recorder

- **WHEN** a caller records a finite task with SnapshotRecording
- **THEN** current values, skips, failures, sharing, and finite wire semantics are preserved through its common SnapshotRecorder handle

#### Scenario: Preserve equal stream emissions

- **WHEN** one producer admits two outputs with equal values
- **THEN** the common recorder receives two emission notifications with distinct real message/ordinal identities
- **AND** historical recording does not collapse those notifications by value equality

#### Scenario: Observe a stream without a telemetry backend

- **WHEN** a caller attaches an unbound SnapshotRecorder callback to a stream
- **THEN** it receives original values at invocation and admitted publication boundaries without requiring an exporter or retaining runtime history

## MODIFIED Requirements

### Requirement: Enable streaming business data capture explicitly

Streaming data capture SHALL require an explicit request through runner configuration or a
programmatic snapshot callback. TUI launch SHALL be permitted to request it for its child.
Disabled capture MUST NOT serialize business values. Unavailable export support SHALL report
an actionable capture diagnostic without changing business execution. Run-bound callbacks
SHALL require matching stream observation metadata before startup. Unbound snapshot callbacks
SHALL remain usable without telemetry providers or exporters.

#### Scenario: Execute without data capture

- **WHEN** a streaming runner executes normally or with verbose lifecycle diagnostics but no stream data request
- **THEN** its automatic telemetry contains no input/output business values and it allocates no data recorder

#### Scenario: Request capture without export support

- **WHEN** stream data is requested from a telemetry-disabled runner or without an available OTLP logs exporter
- **THEN** capture is reported unavailable while the workflow retains its normal results and exit outcome

#### Scenario: Attach mismatched programmatic capture

- **WHEN** a run-bound recorder is attached without matching stream observation metadata
- **THEN** configuration validation fails before source admission or node execution
