## MODIFIED Requirements

### Requirement: Correlate opt-in snapshots with active spans

Opt-in snapshot LogRecords emitted under a valid active span SHALL preserve its native trace and span IDs. All fragments of one snapshot record SHALL preserve the same association. Snapshot sequence, digest, and workflow/run identities SHALL retain their meanings independently of the self-contained payload format. Correlation MUST NOT require embedding full business values in span attributes.

#### Scenario: Export node input and output snapshots

- **WHEN** an observed finite node emits opt-in snapshots while its span context is attached
- **THEN** exported snapshot LogRecords identify that node invocation's trace and span

#### Scenario: Fragment a large node record

- **WHEN** a snapshot record exceeds the single-packet limit
- **THEN** every exported fragment retains the record's active trace/span association

#### Scenario: Export without an active span

- **WHEN** a snapshot is emitted without valid trace context
- **THEN** it retains the normal snapshot contract without inventing a trace/span identity

### Requirement: Share runtime snapshot notifications across execution modes

Finite and streaming execution SHALL share the opt-in SnapshotRecorder callback
and ordinary-task capture boundary. The runtime recorder SHALL only notify current
node snapshots and scope. It MUST NOT contain stream identity, run binding, dictionaries,
or historical roots. Observation publication SHALL own encoding and diagnostics;
the receiver SHALL own historical retention and replay. The runtime MUST NOT keep
a separate stream data observer or duplicate task capture path.

#### Scenario: Observe a finite task through the common recorder

- **WHEN** a caller observes a finite task through SnapshotRecorder
- **THEN** redacted current values, skips, failures, public subtree sharing, and finite wire semantics are preserved through its common SnapshotRecorder handle

#### Scenario: Preserve equal stream emissions

- **WHEN** one producer admits two outputs with equal values
- **THEN** the common recorder receives two emission notifications under the existing active span context
- **AND** historical recording does not collapse those notifications by value equality

#### Scenario: Observe a stream without a telemetry backend

- **WHEN** a caller attaches a SnapshotRecorder callback to a stream
- **THEN** it receives values with secret subtrees redacted at invocation and admitted publication boundaries without requiring an exporter or retaining runtime history

### Requirement: Reuse the complete snapshot pipeline across execution modes

Finite and streaming runners SHALL share MF_CAPTURE_SNAPSHOTS, SnapshotPublisher,
the snapshot protocol, exporter, and receiver-owned history. Runtime SHALL
only publish common notifications. Missing records SHALL leave explicit incomplete
history rather than requiring stream-only gap recovery, omission, or eviction policies.

#### Scenario: Capture either execution mode

- **WHEN** a finite or streaming runner enables snapshot capture
- **THEN** the same recorder, exporter, and decoder handle its node snapshots

#### Scenario: Lose a snapshot record

- **WHEN** ordered reconstruction detects a missing record
- **THEN** previously reconstructed history stays inspectable and capture is marked incomplete

#### Scenario: Disable capture

- **WHEN** snapshot capture is disabled
- **THEN** neither execution mode creates a business-data recording or exporter

#### Scenario: Preserve snapshot metadata and values

- **WHEN** either mode exports snapshots
- **THEN** native context, timestamps, sequence order, numeric types, and distinct equal emissions are preserved

### Requirement: Redact secret values before observation

Runtime snapshot callbacks SHALL replace marked subtrees with the fixed JSON string
[REDACTED] before observers receive values, publish records, retain history, or export
through OTel. Public fields SHALL retain their values. Finite and streaming capture,
including nested scopes, SHALL share this rule. Display and Debug SHALL use the same projection.

#### Scenario: Inspect mixed public and sensitive data

- **WHEN** a snapshot contains public fields and marked scalar or container values
- **THEN** callbacks, recording sinks, retained history, and reconstructed snapshots contain placeholders for the secret subtrees and original public data
- **AND** secret containers expose no keys, lengths, descendants, payload hashes, or prefixes

#### Scenario: Record equal public and secret aliases

- **WHEN** a recorder observes a public value and a marked alias with equal business data in either order
- **THEN** completed-state deduplication MUST NOT substitute the public payload for the secret placeholder or suppress the changed observed state

#### Scenario: Format a secret value for diagnostics

- **WHEN** a caller uses Display or Debug on a value containing secrets
- **THEN** marked subtrees are replaced completely with placeholders
- **AND** business serialization continues to return original JSON data

## ADDED Requirements

### Requirement: Retain snapshot history only at the observation receiver

Publishers SHALL send self-contained node inputs and outputs keyed by port name,
retaining no prior payloads, dictionaries, or historical roots. Receivers SHALL retain
immutable scope/node history, share unchanged nodes, and preserve repeated Started
and Emitted boundaries. Both sides SHALL validate the current record and transport
format. Earlier unreleased formats impose no compatibility requirements.

#### Scenario: Publish many distinct node values

- **WHEN** a run publishes successive distinct outputs
- **THEN** each node record contains the complete values and publication releases prior payloads
- **AND** no value IDs or eviction policy are required at the producer

#### Scenario: Inspect overwritten node values

- **WHEN** a node publishes a new value in the same scope
- **THEN** the receiver exposes the new current value and older historical roots keep the prior value

#### Scenario: Reject malformed self-contained records

- **WHEN** node inputs or outputs are not objects
- **THEN** the publisher reports a typed diagnostic without sending the invalid node record
- **AND** the receiver rejects invalid external records before changing historical state

#### Scenario: Preserve numeric payloads

- **WHEN** node values contain unsigned 64-bit integers, integers, floats, or negative zero
- **THEN** direct and fragmented OTLP records preserve their numeric representation

#### Scenario: Reject an unsupported format

- **WHEN** a snapshot transport or header does not match the current format
- **THEN** capture rejects it explicitly before applying node state
