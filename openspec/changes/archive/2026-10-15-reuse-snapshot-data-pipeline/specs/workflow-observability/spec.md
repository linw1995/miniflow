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

### Requirement: Gate unsupported streaming snapshot capture

**Reason**: Both execution modes now use the shared snapshot pipeline.

**Migration**: Enable MF_CAPTURE_SNAPSHOTS for either runner mode; no separate streaming capture flag is required.

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

### Requirement: Preserve isolated failure and snapshot boundaries

Selected execution SHALL preserve preparation, input, invocation, publication, and result-selection
failure phases. It MUST NOT emit a successful target start for rejected bindings or failed preparation.
Unrelated provider preparation failures MUST NOT count as target execution. Opt-in snapshots SHALL follow
the selected finite operation, including Task selection from stream binaries. Export or snapshot failures
MUST retain ordinary bounded cleanup and business-result semantics.

#### Scenario: Reject invalid isolated context

- **WHEN** selected context validation fails before invocation
- **THEN** any emitted failure identifies input or preparation context and no target-start event is emitted

#### Scenario: Fail unrelated factory preparation

- **WHEN** whole-plan preparation fails in a provider outside the selected operation
- **THEN** observation reports preparation failure without inventing a lifecycle for that provider or the target

#### Scenario: Capture finite task values from a streaming executable

- **WHEN** a Task selected from a stream binary requests snapshots with an available logs exporter
- **THEN** its finite snapshots use the selected-operation identity and normal bounded capture behavior

#### Scenario: Keep whole-stream snapshot restrictions

- **WHEN** the stream binary is invoked without selection and requests whole-stream snapshots
- **THEN** it uses the shared snapshot backend with the same opt-in and exporter prerequisites as finite execution

#### Scenario: Preserve results during export failure

- **WHEN** an isolated Task succeeds but telemetry export is unavailable
- **THEN** the runner preserves its validated result and success status while reporting bounded export diagnostics
