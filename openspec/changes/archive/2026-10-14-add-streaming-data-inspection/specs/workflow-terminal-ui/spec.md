## MODIFIED Requirements

### Requirement: Keep streaming display independent of snapshot capture

The TUI SHALL explicitly disable whole-run value snapshots for streaming children, including inherited capture settings, while retaining lifecycle, trace, and log display. It SHALL request the separate bounded streaming data mode for streaming children and present its received data in the Data pane. Missing capture support SHALL produce an explicit availability diagnostic. Supported finite workflows SHALL retain snapshot capture and expose their decoded inputs/outputs in the observation view.

#### Scenario: Launch with inherited snapshot settings

- **WHEN** the parent requests snapshot capture and the TUI launches a streaming workflow
- **THEN** the child runs with whole-run snapshot capture disabled, bounded stream data requested, and lifecycle, spans, and logs observable

#### Scenario: Inspect a finite workflow's values

- **WHEN** a supported finite workflow is launched in TUI mode
- **THEN** its value snapshots are captured and browsable through the corresponding node span's Data pane

#### Scenario: Inspect an older compatible streaming runner

- **WHEN** a compatible lifecycle-schema-4 runner has no bounded stream data support
- **THEN** Details and Logs remain usable and Data reports absent capture support/evidence without claiming empty successful capture

## ADDED Requirements

### Requirement: Inspect streaming data as an invocation list

The Data pane SHALL list a selected stream node span's exact trace/span-correlated inputs and admitted outputs in capture-sequence order. Rows SHALL identify direction, ports, trigger, applicable message, emission ordinal, time, and a bounded summary. Selection SHALL reveal decoded values or omission reasons. Outcomes and skipped ports MUST retain their lifecycle meanings rather than being inferred from payload records.

#### Scenario: Select a producer with multiple emissions

- **WHEN** a producer's span has one input record and several output records
- **THEN** Data presents distinct selectable rows and each expanded output belongs to its own emission

#### Scenario: Receive data after selecting a span

- **WHEN** matching data arrives after the selected span's Data pane was rendered
- **THEN** the list and current capture diagnostics refresh without changing span selection

#### Scenario: Distinguish identical node names

- **WHEN** two invocations with the same node name have different values
- **THEN** each selected span lists only its exact correlated invocation data

#### Scenario: Inspect zero-emission or failed work

- **WHEN** an invocation buffers without emitting or fails after admitting an output
- **THEN** its list preserves the observed input/output records without fabricating emissions or a successful outcome

### Requirement: Browse streaming data before spans complete

OTEL Tracing SHALL provide a session-wide streaming Data list independent of completed or retained spans. It SHALL admit only the session's workflow/run records and show explicit invocation correlation. Missing native context SHALL remain visible without guessed span association. Arrival of later sequences SHALL NOT wait for earlier data. Retained data SHALL remain inspectable after collection ends.

#### Scenario: Inspect a live autonomous producer

- **WHEN** a long-running producer emits values before its span is exported
- **THEN** its received values are inspectable in session Data while execution continues

#### Scenario: Sample or evict a span

- **WHEN** an admitted data record's span is sampled out or removed by raw-span retention
- **THEN** the record remains available in session Data under its explicit invocation identity

#### Scenario: Receive unrelated data

- **WHEN** data identifies another workflow or run
- **THEN** it does not enter the current session's Data list or change its capture status

#### Scenario: Receive data out of order

- **WHEN** output sequence 5 arrives before input sequence 4
- **THEN** sequence 5 is inspectable immediately and later admission of sequence 4 orders both rows without losing selection

#### Scenario: Inspect completed streaming execution

- **WHEN** the runner exits and bounded telemetry draining ends
- **THEN** remaining Data rows and capture diagnostics stay available until the user closes the final view

### Requirement: Keep data selection stable during live updates

Data selection SHALL follow record identity across insertions and reordering. New records SHALL follow the tail until the user navigates to older data, and an explicit action SHALL resume following. Eviction of the selected record SHALL be reported and select the nearest retained row. Value previews and row labels SHALL have finite limits and escape terminal control characters.

#### Scenario: Browse while new values arrive

- **WHEN** a user selects an older record while the stream emits new values
- **THEN** that record remains selected until the user resumes follow or it is evicted

#### Scenario: Evict the selected value

- **WHEN** retention removes the selected record
- **THEN** the UI reports the eviction and chooses a retained neighbor without retaining the evicted payload indefinitely

#### Scenario: Truncate only the preview

- **WHEN** a retained value is within capture limits but exceeds the display preview limit
- **THEN** Data marks preview truncation while its store retains the admitted value

### Requirement: Bound streaming data retention separately from other signals

The Data store, indexes, deduplication witnesses, gap bookkeeping, and diagnostics SHALL have finite count/byte limits. Records SHALL decode independently and old payloads SHALL be released on eviction. Capture omissions, sender drops, transport uncertainty, local evictions, and typed decoding/conflict faults SHALL remain distinguishable. Data limits or faults MUST NOT stop execution, lifecycle reduction, finite snapshots, or raw detail browsing.

#### Scenario: Run beyond the data retention budget

- **WHEN** a stream produces more data than the configured record or byte budget
- **THEN** oldest data is evicted with visible local accounting and total retained memory remains bounded

#### Scenario: Retransmit a retained or retired record

- **WHEN** an identical record is redelivered inside the witness window or an ancient sequence is redelivered outside it
- **THEN** no duplicate row is added and retired evidence is not claimed to have been reverified

#### Scenario: Receive a conflicting or malformed record

- **WHEN** an incoming record conflicts with retained evidence or fails data decoding
- **THEN** a bounded typed diagnostic is retained while previously admitted data and later independent valid records remain usable

#### Scenario: Exceed unresolved gap capacity

- **WHEN** capture gaps exceed bounded verification bookkeeping
- **THEN** Data retains an uncertainty diagnostic without allocating an unbounded gap history or claiming complete capture

#### Scenario: Saturate raw log display

- **WHEN** raw LogRecord retention is full and valid stream data arrives
- **THEN** the dedicated Data store can still admit it within its own budgets

#### Scenario: Finish with missing capture evidence

- **WHEN** process completion occurs without a valid capture end record
- **THEN** Data reports an unverified capture tail separately from workflow outcome and lifecycle completeness
