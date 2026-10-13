## MODIFIED Requirements

### Requirement: Gate unsupported streaming snapshot capture

Streaming instances SHALL reject the current whole-run snapshot recorder before startup execution. Generated
runners MUST reject `MF_CAPTURE_SNAPSHOTS=1` in streaming mode with an actionable diagnostic. Ordinary
bounded-run capture SHALL retain its existing behavior. Separately enabled bounded streaming data capture
SHALL NOT use that whole-run recorder or inherit its rejection.

#### Scenario: Request stream snapshot history

- **WHEN** snapshot capture is enabled for a streaming runner or attached programmatically to a streaming instance
- **THEN** startup rejects the unsupported mode before source admission or node execution

#### Scenario: Request bounded stream data

- **WHEN** a streaming run enables the supported bounded data mode without requesting whole-run snapshots
- **THEN** execution can capture streaming inputs and outputs without allocating whole-run snapshot history

## ADDED Requirements

### Requirement: Enable streaming business data capture explicitly

Streaming data capture SHALL require an explicit request through runner configuration or a programmatic capture option. TUI launch SHALL be permitted to request it for its child. Disabled capture MUST NOT serialize business values. Unavailable export support SHALL report an actionable capture diagnostic without changing business execution. Programmatic capture SHALL require a matching stream observation before startup.

#### Scenario: Execute without data capture

- **WHEN** a streaming runner executes normally or with verbose lifecycle diagnostics but no stream data request
- **THEN** its automatic telemetry contains no input/output business values and it allocates no data recorder

#### Scenario: Request capture without export support

- **WHEN** stream data is requested from a telemetry-disabled runner or without an available OTLP logs exporter
- **THEN** capture is reported unavailable while the workflow retains its normal results and exit outcome

#### Scenario: Attach mismatched programmatic capture

- **WHEN** a recorder is attached without a matching stream observation
- **THEN** configuration validation fails before source admission or node execution

### Requirement: Capture resolved inputs and admitted streaming outputs

Enabled capture SHALL record resolved input ports before node invocation and each successfully admitted output separately after publication validation. Inputs and outputs SHALL retain their actual invocation and trigger. Zero emissions MUST NOT invent outputs. An admitted emission SHALL remain inspectable even if its producer or callback later fails. Rejected publications MUST NOT appear as admitted outputs.

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
- **THEN** the output identifies its actual trigger and newly allocated output message without inventing an input message

#### Scenario: Capture nested synchronous work

- **WHEN** a stream message executes Loop or Iteration body tasks with capture enabled
- **THEN** their resolved inputs and published outputs are captured under their own invocation/span context

### Requirement: Correlate stream data independently of node names

Data records SHALL identify workflow/run, node scope, stream invocation, trigger, and applicable input message. Each emission SHALL identify its output message and invocation-local ordinal. Valid active native trace/span context SHALL be preserved. Repeated node names, Loop paths, or Iteration indices MUST NOT alias across invocations. Missing native context MUST NOT cause guessed span association.

#### Scenario: Emit multiple output messages

- **WHEN** one invocation emits several results into an output domain
- **THEN** their records share the emitting invocation but retain distinct output message identities and ordinals

#### Scenario: Repeat nested paths across messages

- **WHEN** two stream messages execute identical Loop paths or Iteration item indices
- **THEN** their data remains distinguishable by invocation and native context

#### Scenario: Export data without a retained span

- **WHEN** a data record arrives without usable native context or its span is not retained
- **THEN** its explicit stream identity remains available without assigning it to another span

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

- **WHEN** the next sequence or emission ordinal cannot be represented without overflow
- **THEN** capture stops with a diagnostic instead of wrapping identity or changing workflow results

### Requirement: Bound streaming data capture independently of execution

Encoding, queued records/bytes, diagnostics, and retained state SHALL have finite budgets independent of stream age. Oversized or excessive-depth values SHALL produce explicit bounded omission metadata when delivery permits. Capture admission SHALL be nonblocking; overflow or export failure MUST NOT change results, execution order, source capacity semantics, or workflow outcome. Shutdown SHALL use finite flush deadlines.

#### Scenario: Saturate the data queue

- **WHEN** data export cannot keep up with successful producer sends
- **THEN** capture drops data with bounded accounting while the producer retains its existing business admission behavior

#### Scenario: Exceed the value limit

- **WHEN** an input or emission exceeds capture encoding limits
- **THEN** a bounded record identifies the omitted data and reason while other data and workflow execution remain usable

#### Scenario: Lose the collector

- **WHEN** the collector becomes unreachable during stream execution or shutdown
- **THEN** typed export failures remain available for diagnostics and execution/cleanup do not wait indefinitely

### Requirement: Expose streaming capture loss without claiming complete history

Capture SHALL reserve sequence numbers before encoding or enqueueing and publish bounded start/status/end evidence with cumulative omissions and producer drops when possible. Consumers SHALL distinguish sender loss, retained transport gaps, local eviction, and capture faults. Missing final evidence or retired unresolved gaps SHALL leave data completeness unverified. Data faults MUST NOT alter lifecycle integrity.

#### Scenario: Drop an interior record

- **WHEN** a reserved record is dropped before export and a later sequence is received
- **THEN** capture loss remains observable without requesting replay or re-executing nodes

#### Scenario: Receive no capture boundary

- **WHEN** the process succeeds but no capture start or end evidence is received
- **THEN** consumers cannot infer that data capture was available or complete

#### Scenario: Close with producer drops

- **WHEN** an end record arrives after producer-side queue drops
- **THEN** it exposes cumulative sender drops without presenting the retained data as complete or double-counting overlapping gaps

#### Scenario: Reject an unsupported data version

- **WHEN** a consumer receives an unknown streaming data envelope version
- **THEN** it reports a Data compatibility diagnostic while supported lifecycle and raw detail remain usable
