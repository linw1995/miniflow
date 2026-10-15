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

## MODIFIED Requirements

### Requirement: Keep streaming display independent of snapshot capture

The TUI SHALL enable the common snapshot flag for both execution modes, overriding inherited
capture settings. Lifecycle, trace, and log display SHALL remain usable when snapshot capture
is unavailable. Data SHALL report missing capture support or evidence rather than claiming
successful empty capture.

#### Scenario: Launch with inherited snapshot settings

- **WHEN** inherited settings disable snapshots and the TUI launches either execution mode
- **THEN** the child receives MF_CAPTURE_SNAPSHOTS=1 and lifecycle, spans, and logs remain observable

#### Scenario: Inspect a finite workflow's values

- **WHEN** a supported finite workflow is launched in TUI mode
- **THEN** the shared snapshot pipeline supplies its invocation and session Data lists

#### Scenario: Inspect an older compatible streaming runner

- **WHEN** a compatible lifecycle-schema-4 runner lacks shared snapshot support
- **THEN** Details and Logs remain usable and Data reports absent capture evidence without claiming complete empty history
