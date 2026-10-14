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
