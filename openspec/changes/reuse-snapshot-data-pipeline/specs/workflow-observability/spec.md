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
