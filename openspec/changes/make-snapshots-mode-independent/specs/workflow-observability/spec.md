## MODIFIED Requirements

### Requirement: Enable streaming business data capture explicitly

Streaming data capture SHALL require an explicit request through runner configuration or a
programmatic snapshot callback. TUI launch SHALL be permitted to request it for its child.
Disabled capture MUST NOT serialize business values. Unavailable export support SHALL report
an actionable capture diagnostic without changing business execution. Programmatic snapshot callbacks
SHALL remain usable without telemetry providers, exporters, or run binding.

#### Scenario: Execute without data capture

- **WHEN** a streaming runner executes normally or with verbose lifecycle diagnostics but no stream data request
- **THEN** its automatic telemetry contains no input/output business values and it allocates no data recorder

#### Scenario: Request capture without export support

- **WHEN** stream data is requested from a telemetry-disabled runner or without an available OTLP logs exporter
- **THEN** capture is reported unavailable while the workflow retains its normal results and exit outcome

#### Scenario: Attach raw programmatic capture

- **WHEN** a snapshot callback is attached without a stream observation
- **THEN** it receives original values at invocation and admitted publication boundaries

### Requirement: Capture resolved inputs and admitted streaming outputs

Enabled capture SHALL record resolved input ports before node invocation and each successfully admitted output separately after publication validation. Inputs and outputs SHALL retain their node scope and valid active span context. Zero emissions MUST NOT invent outputs. An admitted emission SHALL remain inspectable even if its producer or callback later fails. Rejected publications MUST NOT appear as admitted outputs.

#### Scenario: Inspect a producer before completion

- **WHEN** one producer successfully sends three results and remains running
- **THEN** capture offers its inputs and three distinct output records without waiting for its terminal event

#### Scenario: Reject an invalid send

- **WHEN** a producer attempts to send an invalid result after an earlier valid emission
- **THEN** only the valid result appears as admitted output and the existing publication failure remains authoritative

#### Scenario: Fail after partial callback publication

- **WHEN** an event callback admits an emission before a later emission or timer update fails
- **THEN** capture retains the admitted output without describing the callback as successful

#### Scenario: Buffer without flushing

- **WHEN** a Batch input callback accepts an item and emits no output
- **THEN** capture records its input ports without fabricating an empty batch or downstream success

#### Scenario: Flush on a timer or upstream close

- **WHEN** a callback without an input message admits a batch output
- **THEN** capture records the actual admitted output under its callback span without inventing input values

#### Scenario: Capture nested synchronous work

- **WHEN** a stream message executes Loop or Iteration body tasks with capture enabled
- **THEN** their resolved inputs and published outputs are captured under their own invocation/span context

### Requirement: Correlate stream data independently of node names

Data records SHALL preserve workflow/run, node scope, valid active native trace/span
context, timestamp, and checked capture sequence. Invocation association SHALL use
native trace/span IDs. Equal-valued emissions SHALL retain separate capture records.
Missing native context MUST NOT cause guessed association; timestamps MUST NOT serve
as unique identities. Snapshot notifications MUST NOT carry a separate stream identity.

#### Scenario: Emit multiple equal outputs

- **WHEN** one invocation emits several equal-valued results
- **THEN** records share the native span context and retain distinct capture sequences

#### Scenario: Repeat nested paths across messages

- **WHEN** two invocations execute identical Loop paths or Iteration item indices
- **THEN** their data remains distinguishable by native trace/span context

#### Scenario: Export data without native context

- **WHEN** a data record has no usable native context or its span is not retained
- **THEN** its node scope, values, timestamp, and sequence remain inspectable in session Data without assignment to another span

### Requirement: Decode retained stream data without historical dependencies

Streaming data SHALL use a separately versioned OTLP logs protocol with self-contained values and checked capture sequences independent of lifecycle sequences. Values SHALL preserve integer/float distinctions and unsigned precision. Missing, evicted, or malformed earlier data MUST NOT prevent decoding later valid records. Finite snapshot formats and lifecycle versions SHALL retain their meanings.

#### Scenario: Lose a prior value record

- **WHEN** an input record is lost or evicted and a later output record arrives
- **THEN** the output can be decoded without a value definition or contiguous history from the input record

#### Scenario: Preserve numeric types

- **WHEN** capture exports the maximum unsigned 64-bit integer and a floating-point value representing an integer
- **THEN** their decoded values preserve unsigned precision and their original numeric types

#### Scenario: Receive data out of order

- **WHEN** a later capture sequence arrives before an earlier one
- **THEN** the later record is usable immediately and the earlier record can subsequently be admitted

#### Scenario: Exhaust the capture counter

- **WHEN** the next capture sequence cannot be represented without overflow
- **THEN** capture stops with a diagnostic instead of wrapping the sequence or changing workflow results

### Requirement: Keep streaming capture policy outside runtime execution

The runtime SHALL provide current resolved input values and successfully admitted
output values through the common opt-in snapshot callback with node scope. It SHALL NOT encode stream data records, impose telemetry
byte/depth budgets, maintain capture sequences or loss counters, or retain
inspection history. Telemetry SHALL own capture encoding and diagnostics, runner
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

### Requirement: Share runtime snapshot notifications across execution modes

Finite and streaming execution SHALL share the opt-in SnapshotRecorder callback
and ordinary-task capture boundary. The runtime recorder SHALL only notify current
node snapshots and scope. It MUST NOT contain stream identity or run binding. Caller-owned recording SHALL
manage dictionaries, encoding, history, and diagnostics. The runtime MUST NOT keep
a separate stream data observer or duplicate task capture path.

#### Scenario: Observe a finite task through the common recorder

- **WHEN** a caller records a finite task with SnapshotRecording
- **THEN** current values, skips, failures, sharing, and finite wire semantics are preserved through its common SnapshotRecorder handle

#### Scenario: Preserve equal stream emissions

- **WHEN** one producer admits two outputs with equal values
- **THEN** the common recorder receives two emission notifications under the existing active span context
- **AND** historical recording does not collapse those notifications by value equality

#### Scenario: Observe a stream without a telemetry backend

- **WHEN** a caller attaches a SnapshotRecorder callback to a stream
- **THEN** it receives original values at invocation and admitted publication boundaries without requiring an exporter or retaining runtime history
