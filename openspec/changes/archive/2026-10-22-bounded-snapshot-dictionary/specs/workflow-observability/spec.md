## ADDED Requirements

### Requirement: Bound producer snapshot dictionary retention

SnapshotRecording SHALL default to unbounded dictionary retention and SHALL accept an
opt-in bounded retention with a maximum retained value count. Bounded retention SHALL
evict least-recently-used live deduplication entries and redefine an evicted value
under a new value ID when it reappears. The wire format, protocol version, receiver
store, and history contracts SHALL remain unchanged, and the receiver SHALL retain the
complete stream including redefinitions.

#### Scenario: Default to unbounded retention

- **WHEN** a recording is created without a retention limit
- **THEN** its deduplication dictionary retains every distinct value for the session

#### Scenario: Redefine an evicted value

- **WHEN** bounded retention evicts a live dictionary entry and the value reappears
- **THEN** the producer defines it again under a new value ID and the receiver reconstructs the complete stream

#### Scenario: Preserve assigned-ID accounting

- **WHEN** a bounded recording assigns value IDs across redefinitions
- **THEN** the total assigned-ID count remains observable, keeps growing, and matches the receiver's stored value count

## MODIFIED Requirements

### Requirement: Share runtime snapshot notifications across execution modes

Finite and streaming execution SHALL share the opt-in SnapshotRecorder callback
and ordinary-task capture boundary. The runtime recorder SHALL only notify current
node snapshots and scope. It MUST NOT contain stream identity or run binding. Caller-owned recording SHALL
manage dictionaries, encoding, history, and diagnostics; it MAY bound its live
deduplication dictionary with opt-in retention, redefining evicted values under new
value IDs without changing the wire format or receiver retention. The runtime MUST NOT keep
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

### Requirement: Reuse the complete snapshot pipeline across execution modes

Finite and streaming runners SHALL share MF_CAPTURE_SNAPSHOTS, SnapshotRecording,
the snapshot protocol, exporter, receiver, and caller-owned history. Runtime SHALL
only publish common notifications. Missing records SHALL leave explicit incomplete
history rather than requiring stream-only gap recovery, omission, or eviction policies.
Generated runners SHALL read MF_SNAPSHOT_DICTIONARY_LIMIT and enable bounded producer
dictionary retention for a valid value; absent or invalid values SHALL retain unbounded
retention, and invalid values SHALL produce a diagnostic without changing capture
behavior.

#### Scenario: Capture either execution mode

- **WHEN** a finite or streaming runner enables snapshot capture
- **THEN** the same recorder, exporter, and decoder handle its node snapshots

#### Scenario: Lose a snapshot definition

- **WHEN** ordered reconstruction cannot resolve a missing record or definition
- **THEN** previously reconstructed history stays inspectable and capture is marked incomplete

#### Scenario: Disable capture

- **WHEN** snapshot capture is disabled
- **THEN** neither execution mode creates a business-data recording or exporter

#### Scenario: Preserve snapshot metadata and values

- **WHEN** either mode exports snapshots
- **THEN** native context, timestamps, sequence order, numeric types, and distinct equal emissions are preserved

#### Scenario: Bound the runner producer dictionary

- **WHEN** a generated runner enables snapshot capture with a valid MF_SNAPSHOT_DICTIONARY_LIMIT
- **THEN** its producer dictionary is bounded and evicted values are redefined under new value IDs while the receiver retains the complete stream

#### Scenario: Fall back on a missing or invalid dictionary limit

- **WHEN** MF_SNAPSHOT_DICTIONARY_LIMIT is absent or not a valid value
- **THEN** capture retains the full dictionary and an invalid value produces a diagnostic without changing the workflow result
