## REMOVED Requirements

### Requirement: Inspect streaming data as an invocation list

**Reason**: Replace stream-only presentation and retention with the common snapshot history.

**Migration**: Inspect session or exact-span snapshot entries through the shared Data list.

### Requirement: Browse streaming data before spans complete

**Reason**: Replace stream-only presentation and retention with the common snapshot history.

**Migration**: Inspect session or exact-span snapshot entries through the shared Data list.

### Requirement: Keep data selection stable during live updates

**Reason**: Replace stream-only presentation and retention with the common snapshot history.

**Migration**: Inspect session or exact-span snapshot entries through the shared Data list.

### Requirement: Bound streaming data retention separately from other signals

**Reason**: Replace stream-only presentation and retention with the common snapshot history.

**Migration**: Inspect session or exact-span snapshot entries through the shared Data list.

## ADDED Requirements

### Requirement: Browse shared snapshot history as Data lists

Both execution modes SHALL share session and exact native trace/span Data lists
over reconstructed snapshot history. Rows SHALL identify node, outcome, time, and
sequence; selection SHALL reveal decoded snapshot values. Capture diagnostics and
history SHALL remain inspectable after execution. History retention SHALL be
caller-owned without stream-only eviction or omission policies.

#### Scenario: Inspect an active producer

- **WHEN** an active producer publishes several equal admitted outputs before its span ends
- **THEN** session Data presents distinct reconstructed snapshot entries before span export

#### Scenario: Inspect repeated invocations

- **WHEN** invocations share a node name but have different native trace/span context
- **THEN** selecting a span shows only its own snapshot entries

#### Scenario: Preserve selection

- **WHEN** a user selects an older entry and more snapshots arrive
- **THEN** that entry stays selected until the user resumes following

#### Scenario: Inspect incomplete history

- **WHEN** collection ends without complete reconstruction
- **THEN** the common capture diagnostic and previously reconstructed entries remain inspectable

#### Scenario: Inspect missing context and large values

- **WHEN** a snapshot lacks native context or exceeds preview limits
- **THEN** it remains in session history without guessed association or truncating stored values
