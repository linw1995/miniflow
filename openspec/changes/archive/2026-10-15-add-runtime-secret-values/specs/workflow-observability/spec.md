## ADDED Requirements

### Requirement: Redact secret values before observation

Snapshot callbacks and direct recording SHALL replace marked subtrees with the
fixed JSON string [REDACTED] before callbacks, deduplication, dictionary encoding,
retention, or OTel export. Public fields SHALL retain their values. Finite and
streaming capture, including nested scopes, SHALL share this rule without changing
snapshot protocol version 1. Display and Debug SHALL use the same projection.

#### Scenario: Inspect mixed public and sensitive data

- **WHEN** a snapshot contains public fields and marked scalar or container values
- **THEN** callbacks, recording sinks, retained history, and reconstructed snapshots contain placeholders for the secret subtrees and original public data
- **AND** secret containers expose no keys, lengths, descendants, payload hashes, or prefixes

#### Scenario: Record equal public and secret aliases

- **WHEN** a recorder observes a public value and a marked alias with equal business data in either order
- **THEN** dictionary aliasing and completed-state deduplication MUST NOT substitute the public payload for the secret placeholder or suppress the changed observed state

#### Scenario: Format a secret value for diagnostics

- **WHEN** a caller uses Display or Debug on a value containing secrets
- **THEN** marked subtrees are replaced completely with placeholders
- **AND** business serialization continues to return original JSON data

## MODIFIED Requirements

### Requirement: Keep streaming capture policy outside runtime execution

The runtime SHALL provide current resolved input values and successfully admitted
output values through the common opt-in snapshot callback with node scope. It SHALL NOT encode stream data records, impose telemetry
byte/depth budgets, maintain capture sequences or loss counters, or retain
inspection history. Caller-owned recording and exporters SHALL own encoding and diagnostics, runner
owners SHALL finish capture, and consumers SHALL manage historical retention.

#### Scenario: Observe a value above a telemetry budget

- **WHEN** a programmatic callback observes a valid input or admitted output exceeding a telemetry encoding budget
- **THEN** the runtime callback receives the public value without omission or truncation; secret subtrees are redacted
- **AND** a separately attached telemetry adapter may apply its own export policy

#### Scenario: Finish owner-managed capture

- **WHEN** a runner finishes or fails stream execution
- **THEN** its owner finishes telemetry capture and reports capture diagnostics separately from the business outcome

#### Scenario: Inspect admitted output without blocking the scheduler

- **WHEN** a producer or event callback admits an output
- **THEN** the snapshot callback receives its admitted values with secret subtrees redacted after the scheduler lock is released

### Requirement: Share runtime snapshot notifications across execution modes

Finite and streaming execution SHALL share the opt-in SnapshotRecorder callback
and ordinary-task capture boundary. The runtime recorder SHALL only notify current
node snapshots and scope. It MUST NOT contain stream identity or run binding. Caller-owned recording SHALL
manage dictionaries, encoding, history, and diagnostics. The runtime MUST NOT keep
a separate stream data observer or duplicate task capture path.

#### Scenario: Observe a finite task through the common recorder

- **WHEN** a caller records a finite task with SnapshotRecording
- **THEN** redacted current values, skips, failures, public subtree sharing, and finite wire semantics are preserved through its common SnapshotRecorder handle

#### Scenario: Preserve equal stream emissions

- **WHEN** one producer admits two outputs with equal values
- **THEN** the common recorder receives two emission notifications under the existing active span context
- **AND** historical recording does not collapse those notifications by value equality

#### Scenario: Observe a stream without a telemetry backend

- **WHEN** a caller attaches a SnapshotRecorder callback to a stream
- **THEN** it receives values with secret subtrees redacted at invocation and admitted publication boundaries without requiring an exporter or retaining runtime history
