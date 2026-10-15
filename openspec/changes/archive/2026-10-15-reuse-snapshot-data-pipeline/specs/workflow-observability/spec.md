## REMOVED Requirements

### Requirement: Enable streaming business data capture explicitly

**Reason**: The unreleased stream-only pipeline is replaced by the existing snapshot contract in both execution modes.

**Migration**: Use MF_CAPTURE_SNAPSHOTS and the shared snapshot protocol, native context, and incomplete-history diagnostics.

### Requirement: Correlate stream data independently of node names

**Reason**: The unreleased stream-only pipeline is replaced by the existing snapshot contract in both execution modes.

**Migration**: Use MF_CAPTURE_SNAPSHOTS and the shared snapshot protocol, native context, and incomplete-history diagnostics.

### Requirement: Decode retained stream data without historical dependencies

**Reason**: The unreleased stream-only pipeline is replaced by the existing snapshot contract in both execution modes.

**Migration**: Use MF_CAPTURE_SNAPSHOTS and the shared snapshot protocol, native context, and incomplete-history diagnostics.

### Requirement: Bound streaming data capture independently of execution

**Reason**: The unreleased stream-only pipeline is replaced by the existing snapshot contract in both execution modes.

**Migration**: Use MF_CAPTURE_SNAPSHOTS and the shared snapshot protocol, native context, and incomplete-history diagnostics.

### Requirement: Expose streaming capture loss without claiming complete history

**Reason**: The unreleased stream-only pipeline is replaced by the existing snapshot contract in both execution modes.

**Migration**: Use MF_CAPTURE_SNAPSHOTS and the shared snapshot protocol, native context, and incomplete-history diagnostics.

## ADDED Requirements

### Requirement: Reuse the complete snapshot pipeline across execution modes

Finite and streaming runners SHALL share MF_CAPTURE_SNAPSHOTS, SnapshotRecording,
the snapshot protocol, exporter, receiver, and caller-owned history. Runtime SHALL
only publish common notifications. Missing records SHALL leave explicit incomplete
history rather than requiring stream-only gap recovery, omission, or eviction policies.

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

### Requirement: Preserve repeated snapshot observation boundaries

Shared recording and reconstruction SHALL preserve every Started and Emitted notification,
even with equal node values and scope. Completed state updates MAY reuse existing state.
This behavior SHALL be independent of execution mode and require no additional identity.

#### Scenario: Buffer repeated equal inputs

- **WHEN** two callbacks accept equal input values without emitting between them
- **THEN** each Started notification remains a separate inspectable history entry

## MODIFIED Requirements

### Requirement: Keep streaming capture policy outside runtime execution

The runtime SHALL provide current resolved input values and successfully admitted
output values through the common opt-in snapshot callback with node scope. It SHALL NOT encode stream data records, impose telemetry
byte/depth budgets, maintain capture sequences or loss counters, or retain
inspection history. Caller-owned recording and exporters SHALL own encoding and diagnostics, runner
owners SHALL finish capture, and consumers SHALL manage historical retention.

#### Scenario: Observe a value above a telemetry budget

- **WHEN** a programmatic callback observes a valid input or admitted output exceeding a telemetry encoding budget
- **THEN** the runtime callback receives the original value without omission or truncation
- **AND** a separately attached telemetry adapter may apply its own export policy

#### Scenario: Finish owner-managed capture

- **WHEN** a runner finishes or fails stream execution
- **THEN** its owner finishes telemetry capture and reports capture diagnostics separately from the business outcome

#### Scenario: Inspect admitted output without blocking the scheduler

- **WHEN** a producer or event callback admits an output
- **THEN** the snapshot callback receives its admitted original values after the scheduler lock is released
