## ADDED Requirements

### Requirement: Inspect retained execution spans

The TUI SHALL provide a trace tree grouped by trace and parent span identity, with siblings ordered by start time. It SHALL expose span IDs, timing, status, attributes, events, links, resource, and scope metadata. Selection SHALL remain attached to the same span during reordering. Missing parents and malformed cycles MUST NOT hide retained spans.

#### Scenario: Receive a parent after its child

- **WHEN** a child span is received and selected before its parent
- **THEN** receiving the parent reparents the child without changing the selected span

#### Scenario: Retry an exported span

- **WHEN** the same trace/span identity is exported repeatedly
- **THEN** the trace tree retains one span entry

#### Scenario: Inspect absent trace export

- **WHEN** no spans have arrived during execution or after collection ends
- **THEN** the view distinguishes waiting for export from finished collection without inventing execution spans

### Requirement: Inspect span-related and session logs

The observation view SHALL expose retained LogRecords with timestamps, severity, names, bodies, attributes, trace context, resource, and scope metadata. Span-related logs SHALL match both trace and span IDs. Session-wide browsing SHALL remain available for logs without a retained span. Span events SHALL remain distinguishable from independent LogRecords.

#### Scenario: Receive logs before spans

- **WHEN** correlated LogRecords arrive before their span
- **THEN** they are available in session logs and become visible in the selected span's Logs pane when the span arrives

#### Scenario: Inspect a run-level record

- **WHEN** a matching session record lacks a span ID or its span was not retained
- **THEN** the record remains inspectable in session logs without being assigned to an arbitrary node span

#### Scenario: Ignore unrelated telemetry

- **WHEN** a record identifies a different workflow or run
- **THEN** it does not appear in the current session's observation browser

### Requirement: Inspect decoded node data on its span

The Data pane SHALL display the selected finite node invocation's decoded inputs, outputs, outcome, skipped ports, and available errors. Association SHALL use native trace/span context preserved through snapshot assembly. Repeated names in different Loop passes or Iteration items MUST NOT share data. Missing correlation SHALL produce an explicit diagnostic rather than a guessed association.

#### Scenario: Inspect repeated Loop or Iteration work

- **WHEN** two invocations of the same node emit different snapshots
- **THEN** selecting either span shows only that invocation's values

#### Scenario: Receive data after its span

- **WHEN** snapshot records complete after the selected span was already rendered
- **THEN** its Data pane refreshes to show the received values

#### Scenario: Bound an object preview

- **WHEN** a selected input/output object exceeds the preview limit
- **THEN** the pane marks preview truncation while preserving the received value in the snapshot store

### Requirement: Bound raw telemetry detail independently

Raw span and log retention SHALL have finite count and byte budgets, deduplicate retransmissions, and expose display drops separately from lifecycle integrity and snapshot diagnostics. Reaching a raw detail limit MUST NOT stop lifecycle reduction, snapshot reconstruction, or workflow execution. Terminal labels and previews SHALL have finite sizes and safe control-character handling.

#### Scenario: Reach raw log capacity

- **WHEN** additional matching lifecycle or snapshot records arrive after raw log retention is full
- **THEN** raw detail drops increase while lifecycle reduction and snapshot assembly continue normally

#### Scenario: Preserve completed inspection

- **WHEN** the runner exits and the final view remains open
- **THEN** retained spans, logs, and associated data remain inspectable until the user closes the TUI

## MODIFIED Requirements

### Requirement: Keep streaming display independent of snapshot capture

The TUI SHALL explicitly disable whole-run value snapshots for streaming children, including inherited capture settings, while retaining lifecycle, trace, and log display. The Data pane SHALL explain that streaming value snapshots are unavailable. Supported finite workflows SHALL retain snapshot capture and expose their decoded inputs/outputs in the observation view.

#### Scenario: Launch with inherited snapshot settings

- **WHEN** the parent requests snapshot capture and the TUI launches a streaming workflow
- **THEN** the child runs with stream capture disabled and its lifecycle, spans, and logs remain observable

#### Scenario: Inspect a finite workflow's values

- **WHEN** a supported finite workflow is launched in TUI mode
- **THEN** its value snapshots are captured and browsable through the corresponding node span's Data pane

### Requirement: Retain snapshot decoding failures until presentation

Snapshot capture SHALL retain typed transport, JSON record decoding, and store failures across repeated admission attempts. Data presentation SHALL format the retained error into a diagnostic while leaving previously received values available.

#### Scenario: Admit a malformed snapshot record repeatedly

- **WHEN** a snapshot envelope contains an invalid record and another record is subsequently admitted
- **THEN** both admission failures retain the original typed JSON decoding cause and the Data pane displays the failure
