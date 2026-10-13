# workflow-observability Specification

## Purpose

Expose workflow execution through correlated OpenTelemetry traces and lifecycle events so local and external consumers can observe progress without changing execution behavior.

## Requirements

### Requirement: Identify and correlate workflow observations

Lifecycle events SHALL carry an observation schema version, workflow identity, unique run identity,
event timestamp, and monotonically increasing per-run sequence. Node events SHALL include a local
definition node ID, kind, and structured Loop path with pass indices. In single-run protocols, a node
invocation SHALL have
one lifecycle identified by run ID, Loop path, and local node ID; repeated passes SHALL be separate
invocations, not retry attempts. Top-level nodes SHALL use an empty Loop path. Applicable events
SHALL carry native OTel trace/span correlation. Workflow identity MUST agree with the runner
description; separate invocations MUST have distinct run identities even when they share a
distributed trace. Existing protocol versions SHALL keep their prior single-invocation
interpretation.

Streaming records SHALL use a new protocol version and additionally identify the invocation sequence and its
domain/message identity when applicable. Startup, timer, and close invocations MUST have distinct invocation
identities even without an input message. Startup activation MUST NOT invent an external input message or a
`%input` node lifecycle. Nested Loop and Iteration observations MUST retain their containing stream invocation
identity so repeated body paths cannot alias across messages.

#### Scenario: Observe repeated invocations

- **WHEN** the same executable runs twice
- **THEN** both runs identify the same embedded workflow, use different run IDs, and independently sequence
  their lifecycle records starting at 1

#### Scenario: Correlate a node event

- **WHEN** an observed node starts execution
- **THEN** its lifecycle event identifies the workflow, run, node, kind, path, and associated OTel trace/span
  context without an attempt number

#### Scenario: Preserve a single execution lifecycle

- **WHEN** a node invocation fails or delivery of its telemetry is retried
- **THEN** observation does not schedule another invocation or introduce a retry attempt

#### Scenario: Correlate repeated body invocations

- **WHEN** a Loop body node runs in two passes
- **THEN** its events share workflow and run identity but have different pass paths and independent lifecycle
  states

#### Scenario: Keep transport duplicates distinct from repeated work

- **WHEN** an event is redelivered and the body node also runs in another pass
- **THEN** the redelivery is deduplicated by sequence while the later pass remains a separate invocation

#### Scenario: Observe repeated stream messages

- **WHEN** one message-consuming node executes for two emitted messages in the same workflow instance
- **THEN** its events share a run and definition ID but identify two separate invocations and messages

#### Scenario: Distinguish transport duplicates from batch invocations

- **WHEN** a batch invocation event is redelivered and another batch later invokes the same node
- **THEN** the duplicate is identified by its existing sequence and the new work retains its own invocation
  and message identity

#### Scenario: Correlate repeated nested work

- **WHEN** two emitted batches each invoke the same Loop or Iteration body
- **THEN** their body observations remain distinguishable through the containing stream invocation even when
  inner paths and item indices match

#### Scenario: Distinguish startup from source output

- **WHEN** a parameterized source starts once and emits multiple messages
- **THEN** its invocation has a startup trigger and its consumers identify their individual emitted messages
  without synthetic `%input` records

### Requirement: Export live lifecycle events independently of completed spans

Observed execution SHALL emit OTel LogRecords named `mf.workflow.started`, `mf.workflow.finished`, `mf.node.started`,
`mf.node.finished`, and `mf.node.skipped` at their defined boundaries. Live lifecycle delivery MUST NOT wait for the
workflow or node span to finish. In TUI mode these events MUST NOT be suppressed by trace sampling or ordinary
diagnostic log filters. Execution spans SHALL provide workflow/node durations and explicit outcomes; failures SHALL have
error status and conditional skips MUST NOT be represented as errors.

#### Scenario: Observe a long-running node

- **WHEN** a node remains executing beyond the interactive export interval and the receiver is available
- **THEN** its start event is delivered while it is still executing, allowing the consumer to display Running before the node span ends

#### Scenario: Preserve live state with trace sampling

- **WHEN** trace sampling omits a node span during an observed TUI run
- **THEN** the node lifecycle records remain eligible for full delivery

### Requirement: Preserve execution boundaries and failure semantics

A workflow observation scope SHALL start before preparation and finish after selected-output extraction or a
handled
failure. A node start event SHALL mean its dependencies resolved and its implementation is about to be
invoked. Node
success MUST follow output validation and publication. Construction and dependency failures SHALL identify
their phase
and node when known, without requiring a preceding node start. Generated and in-memory execution MUST produce
equivalent
lifecycle meanings and retain existing execution order, skip rules, output values, and error precedence.

For streaming execution, the workflow boundary SHALL finish only after drain or failure cleanup. A streaming
node start SHALL identify its startup, input/message, timer, or close trigger; completion follows state
transition and validation of any emissions. Successful buffering with zero emissions MUST NOT imply downstream
execution or workflow completion. Observation MUST distinguish admission from completion of the accepted
input.

#### Scenario: Reject invalid node outputs

- **WHEN** a node returns outputs that fail publication validation
- **THEN** the node and workflow report failure, and no successful node completion is emitted

#### Scenario: Fail during preparation

- **WHEN** node construction fails before node execution begins
- **THEN** the workflow reports preparation failure with the affected node, and no node implementation is
  reported as having started

#### Scenario: Fail on a missing dependency

- **WHEN** one dependency is unexpectedly absent and another is explicitly skipped
- **THEN** dependency resolution reports node failure before invocation rather than a conditional skip

#### Scenario: Fail during selected-output extraction

- **WHEN** all invoked nodes succeed but a required selected workflow output is skipped
- **THEN** the workflow reports output-selection failure while preserving the successful node outcomes

#### Scenario: Remain active during tail drain

- **WHEN** a source ends while a tail batch or its downstream operation remains pending
- **THEN** no successful workflow terminal event is emitted until the tail and selected output delivery have
  completed

#### Scenario: Observe buffering without an output

- **WHEN** Batch accepts one item without a flush
- **THEN** its input callback can finish with zero emissions while downstream nodes remain uninvoked

#### Scenario: Reject invalid startup arguments

- **WHEN** a workflow invocation supplies invalid parameters or lacks a required execution resource
- **THEN** any emitted execution failure identifies the input or preparation phase and no node-start record or
  source read occurs

#### Scenario: Finish a producer before the workflow

- **WHEN** a producer returns after admitting its last emission and a downstream operation is still active
- **THEN** its successful node outcome does not establish workflow completion

### Requirement: Distinguish conditional skips from unreached nodes

A node skipped because of resolved conditional dependencies SHALL emit `mf.node.skipped` without a start event or implementation invocation. The event SHALL identify the causal source node and port. Node terminal metadata SHALL expose produced and explicitly skipped port names without business values. Nodes proven unreached by a handled failure's terminal execution boundary SHALL be classified as NotRun rather than Skipped.

#### Scenario: Observe an unselected branch

- **WHEN** a router activates one output and explicitly skips another
- **THEN** its terminal metadata identifies both port outcomes, and a downstream node skipped by the inactive port identifies that dependency without executing

#### Scenario: Stop after an earlier failure

- **WHEN** workflow execution fails before a later node is visited
- **THEN** the final execution boundary identifies that later node as NotRun without claiming it was conditionally skipped

### Requirement: Make lifecycle loss detectable without requiring recovery

Lifecycle sequence numbers SHALL be assigned before serialization or enqueueing so dropped records
leave detectable gaps. A handled run SHALL emit a lightweight `mf.workflow.finished` record whose
final sequence equals its own sequence, with workflow outcome, available duration, and failure
context. The existing protocol version SHALL retain its visited execution prefix. The Loop-capable
version SHALL report actual visited-step count and the active-frame prefix on failure. Each Loop
pass SHALL have started and finished boundaries, with the finished boundary recording that pass's
visited prefix. No lifecycle record SHALL follow the workflow terminal record. The Loop-capable
event bound SHALL derive from the finite per-run step budget rather than the number of static node
definitions. Consumers MUST NOT require a complete per-node snapshot or retransmission to observe a
run. Telemetry loss MUST NOT trigger node re-execution or change workflow results.

A consumer SHALL expose visible sequence gaps, known local lifecycle drops, and missing terminal
boundaries. It MUST distinguish detected missing records from inability to verify the stream's tail.
Without a valid terminal boundary it MUST NOT claim completeness or an exact total missing count,
including when the process exits successfully or the entire telemetry stream is absent. While
execution is active, completeness SHALL remain unconfirmed; immediate detection of a wholly lost
suffix without later evidence is not required. Future Loop passes that never started SHALL NOT be
classified as skipped invocations. Missing outcomes inside a visited pass SHALL remain unknown even
when a later pass or the workflow succeeds.

Streaming protocol validation SHALL use checked sequence counters and bounded retained detail instead of deriving a total event bound from static node count or the single-run step budget. The terminal record SHALL report final sequence and aggregate execution counts without an unbounded invocation list. Detail eviction MUST be distinguishable from lifecycle transport loss, and no streaming lifecycle event may follow the terminal record.

#### Scenario: Detect an interior loss

- **WHEN** lifecycle sequence 4 is dropped before export and sequence 5 reaches the consumer
- **THEN** the consumer exposes the missing sequence without requesting replay or changing workflow execution

#### Scenario: Detect missing tail evidence

- **WHEN** the process exits and no valid workflow finish record arrives before the drain deadline
- **THEN** the consumer marks the tail unverified and the missing count unknown rather than claiming a complete stream

#### Scenario: Receive no telemetry

- **WHEN** the runner exits successfully but the receiver has accepted no lifecycle records
- **THEN** process success is preserved and observation is visibly unverified rather than complete

#### Scenario: Preserve uncertainty for missing node outcomes

- **WHEN** a valid workflow finish arrives but a visited node's outcome record is missing
- **THEN** workflow outcome and stream gaps are available while that node's outcome remains unknown

#### Scenario: Identify unreached nodes with bounded terminal metadata

- **WHEN** a handled failure's final record proves a suffix of execution steps was never visited
- **THEN** those nodes are classified NotRun without requiring a full node-state snapshot

#### Scenario: Detect a dropped body event

- **WHEN** a body node terminal event is dropped and a later pass event arrives
- **THEN** the consumer exposes the sequence gap and leaves that earlier invocation's outcome unknown

#### Scenario: Stop before a future pass

- **WHEN** a Loop stops by condition, maximum, or exit
- **THEN** the consumer does not invent skipped node invocations for passes that were never scheduled

#### Scenario: Keep missing tail evidence unknown

- **WHEN** a Loop runner exits and no valid workflow finish record arrives before draining ends
- **THEN** the consumer marks the tail unverified and does not claim complete body observations

#### Scenario: Observe a long-lived instance

- **WHEN** a valid stream produces more lifecycle events than a legacy finite-run bound
- **THEN** compatible export continues with bounded buffering and correct sequence identity without retaining all earlier invocations

#### Scenario: Lose a batch completion event

- **WHEN** a node completion for one batch is lost and a later batch completes
- **THEN** the earlier outcome remains unknown and the later success does not repair the missing observation

### Requirement: Export without altering workflow results

Runner export SHALL be disabled without configuration and SHALL support OTLP/HTTP export to a configured local receiver
or external Collector. Export SHALL use bounded buffering and finite network/shutdown timeouts. Disabled export,
unreachable endpoints, queue overflow, and export errors MUST NOT change node invocation order, selected results, or
workflow success/failure. Both success and handled failure paths SHALL attempt bounded final flush after ending
execution spans. Manifest inspection and build validation MUST NOT emit workflow execution events.

#### Scenario: Run without telemetry configuration

- **WHEN** a standalone runner executes with no exporter configured
- **THEN** it opens no telemetry connection and preserves its usual output and exit behavior

#### Scenario: Lose the receiver

- **WHEN** the configured receiver becomes unreachable or the exporter queue fills
- **THEN** workflow execution continues without waiting indefinitely or converting telemetry errors into workflow failures

#### Scenario: Flush a short failed run

- **WHEN** a runner fails before a normal batch interval elapses
- **THEN** it ends the relevant spans and attempts to export buffered failure records within the shutdown deadline

### Requirement: Limit automatically exported data

Automatically generated observation metadata MUST exclude node configuration and input/output business values. It SHALL expose graph identities, states, timing, port names, and failure context needed to explain execution. This exclusion MUST NOT be presented as automatic sanitization of arbitrary plugin-provided diagnostic messages.

#### Scenario: Observe a workflow containing configured credentials

- **WHEN** a node configuration and its returned values contain credentials
- **THEN** generated metadata and lifecycle attributes do not serialize those configuration fields or returned values

### Requirement: Describe Loop structure without business data

A Loop-capable runner description SHALL encode nested body graph structure, scope-local node IDs and
edges, and the protocol version required to interpret repeated invocations. Lifecycle records SHALL
emit `mf.loop.pass.started` and `mf.loop.pass.finished` boundaries, pass counts, and a stop reason
of `condition`, `maximum`, or `exit` on successful Loop completion. Description and lifecycle fields
MUST NOT expose Loop variable values, predicates, node configuration, or business inputs and
outputs. Older runner description and event versions SHALL remain readable according to their
original contracts.

#### Scenario: Describe a nested Loop

- **WHEN** a compiled runner describes a workflow with nested Loops
- **THEN** the description distinguishes each scope and its local edges without invoking plugin factories or revealing configured values

#### Scenario: Observe Loop termination

- **WHEN** a Loop ends after an explicit exit
- **THEN** its completion record identifies the number of passes and `exit` reason without carrying variable values

### Requirement: Observe Iteration at its outer node boundary

The workflow description and lifecycle SHALL identify an Iteration as one node in its containing graph. Its start and finish
SHALL bracket all item execution. Repeated items and body nodes SHALL emit separate OTel spans and logs in the
`mf.iteration` scope without consuming the bounded outer lifecycle sequence. Detail records SHALL identify the
workflow, run, outer iteration node, and input index; body-node records SHALL also identify the inner node and kind.
Item spans SHALL be children of the outer Iteration span, and body-node spans SHALL be children of their item spans,
including in parallel workers. Automatically exported metadata MUST NOT include item or result values.

#### Scenario: Describe a compiled Iteration workflow

- **WHEN** the embedded manifest of a runner containing an Iteration node is inspected
- **THEN** it reports the outer node and edges without expanding repeated body nodes into static lifecycle positions

#### Scenario: Report an item failure

- **WHEN** an item fails under `terminate`
- **THEN** the item and failing body node report failed detail outcomes, the outer Iteration node fails once with an indexed diagnostic, and the workflow ends with a failure

#### Scenario: Continue after an item failure

- **WHEN** a body node fails under `continue_on_error`
- **THEN** its node and item detail records remain failed while the outer Iteration node and workflow can succeed

#### Scenario: Correlate parallel body execution

- **WHEN** multiple items run in parallel
- **THEN** every body-node span has the corresponding item span as parent, every item span has the outer Iteration span as parent, and detail records carry the input index and shared workflow/run identity

#### Scenario: Report a skipped body node

- **WHEN** an inner conditional dependency skips a body node
- **THEN** its detail record identifies the item index, skipped node, and causal source output without reporting a node start

#### Scenario: Preserve outer lifecycle completeness

- **WHEN** Iteration detail logs reach the terminal UI receiver alongside the ordinary workflow lifecycle records
- **THEN** the detail logs do not consume outer sequence numbers or make the outer lifecycle appear incomplete

### Requirement: Report batch flushes without exporting their values

Streaming observation SHALL distinguish buffered item count from emitted batches. Each flush SHALL report the Batch node, output message identity, item count, and reason `size_exceed`, `timeout_exceed`, or `upstream_closed`. Automatic metadata MUST NOT include the collected values or an unbounded list of constituent input identities.

#### Scenario: Explain a partial batch

- **WHEN** a partial buffer flushes on its deadline
- **THEN** observation identifies its item count and `timeout_exceed` reason without serializing the elements

#### Scenario: Keep buffering separate from downstream success

- **WHEN** a batch is sealed but its business operation remains queued
- **THEN** observation records the flush while leaving that downstream invocation pending

### Requirement: Gate unsupported streaming snapshot capture

Streaming instances SHALL reject the current whole-run snapshot recorder before startup execution. Generated
runners MUST reject `MF_CAPTURE_SNAPSHOTS=1` in streaming mode with an actionable diagnostic. Ordinary
bounded-run capture SHALL retain its existing behavior.

#### Scenario: Request stream snapshot history

- **WHEN** snapshot capture is enabled for a streaming runner or attached programmatically to a streaming
  instance
- **THEN** startup rejects the unsupported mode before source admission or node execution

### Requirement: Version source-driven streaming observations

Source-driven streams SHALL use event schema 4 with explicit startup identity. Their terminal counts SHALL
report startup frames, emitted messages, completed frames, and delivered outputs with checked arithmetic and
consistent bounds. Startup frames SHALL be zero or one. Existing finite versions and stream schema 3 MUST
retain their original meanings.

#### Scenario: Count a source-driven run

- **WHEN** a workflow starts once, emits messages, and successfully drains
- **THEN** its final record counts the startup frame and all completed emitted frames without reporting
  fabricated accepted host inputs

#### Scenario: Reject incompatible records

- **WHEN** a receiver expecting source-driven stream schema 4 receives a schema-3 lifecycle record for that
  session
- **THEN** it reports a protocol mismatch rather than interpreting the old counters under the new rules

#### Scenario: Prevent sequence or counter wraparound

- **WHEN** an observation identity or execution counter reaches its representable limit
- **THEN** it cannot wrap, reuse an identity, or claim an invalid successful terminal boundary

### Requirement: Preserve typed observation setup causes

Compiler observation setup SHALL return compiler-owned description errors without formatting them into telemetry contract messages. Structural ordering failures in described Loop bodies SHALL retain the original `WorkflowCompileError` and the body path.

#### Scenario: Describe an invalid Loop body during observation setup

- **WHEN** observation setup describes a Loop body with an invalid edge endpoint
- **THEN** it returns a compiler description error with the Loop path and typed structural ordering cause

### Requirement: Preserve typed observation identifier failures

Run UUID and trace/span hexadecimal parsing SHALL retain the original parser error in the contract error source chain. Validation after successful parsing SHALL continue enforcing canonical spelling, UUID version and variant, and nonzero trace/span identifiers.

#### Scenario: Parse malformed identifiers

- **WHEN** an observation identifier cannot be parsed as a UUID, trace ID, or span ID
- **THEN** callers can inspect the UUID or integer parser error and distinguish the failed identifier field

### Requirement: Preserve snapshot transport and export causes

Snapshot transport and OTLP provider setup, flush, shutdown, and delivery failures SHALL retain typed causes until callers present diagnostics. Snapshot protobuf, sequence, and digest decoding SHALL preserve their decoder or conversion errors. Cached delivery failures SHALL remain inspectable on subsequent exporter calls and take precedence over SDK flush summaries of those failures.

#### Scenario: Assemble malformed snapshot fragments

- **WHEN** a complete fragment payload is not valid protobuf
- **THEN** the transport error retains the original protobuf decoding error

#### Scenario: Decode invalid snapshot metadata

- **WHEN** snapshot sequence or digest metadata cannot be converted to its required type
- **THEN** the transport error retains the integer or slice conversion cause

#### Scenario: Observe a failed snapshot export repeatedly

- **WHEN** snapshot delivery fails and the owner subsequently flushes, emits, or finishes
- **THEN** the returned error retains the cached typed SDK failure rather than its formatted summary

### Requirement: Identify isolated operations separately from whole-flow runs

Selected Task execution SHALL use a deterministic identity derived from the original workflow identity,
exact target ID, and isolated-operation format. Different targets and whole-flow execution MUST have
distinct identities; supplied values MUST NOT affect identity. Events, finite descriptions, and snapshots
SHALL agree on that identity. Observation SHALL retain original workflow and target correlation without
exposing configuration or business values automatically.

#### Scenario: Invoke one target with different values

- **WHEN** the same selected task runs twice with different inputs or context values
- **THEN** observations share its selected-operation identity and have distinct run identities

#### Scenario: Distinguish targets and execution modes

- **WHEN** callers execute the whole flow and select two different tasks from the same binary
- **THEN** the three operation descriptions and their events identify three distinct operations

#### Scenario: Correlate with the containing workflow

- **WHEN** a selected-operation span is exported
- **THEN** metadata identifies its original workflow and exact target without automatically exporting supplied values

### Requirement: Count only selected finite execution

Isolated Task observation SHALL describe the selected task at top-level position zero with no external
execution edges. Lifecycle counts MUST include only that operation and its actual nested work, without
claiming visits or skips for surrounding nodes. Existing Loop pass and Iteration detail meanings SHALL
remain unchanged. A Task selected from a stream binary SHALL use finite observation semantics.

#### Scenario: Observe a middle task

- **WHEN** a selected task originally appears after several predecessors
- **THEN** successful completion reports one visited top-level task and no lifecycle events for its neighbors

#### Scenario: Observe a selected Loop

- **WHEN** a selected Loop executes several passes
- **THEN** it has one top-level lifecycle and its real body invocations retain normal pass paths and step counts

#### Scenario: Select a finite task from a streaming binary

- **WHEN** an ordinary Task is selected from a stream flow
- **THEN** observation reports one finite operation without startup frames, stream message lifecycles, or producer events

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
- **THEN** it retains the existing unsupported streaming capture diagnostic

#### Scenario: Preserve results during export failure

- **WHEN** an isolated Task succeeds but telemetry export is unavailable
- **THEN** the runner preserves its validated result and success status while reporting bounded export diagnostics

### Requirement: Emit local verbose lifecycle diagnostics

Generated runners and `mf run` SHALL accept `-v` and `--verbose` to print existing lifecycle metadata on stderr.
Diagnostics SHALL preserve result stdout and workflow outcomes without automatically dumping node configuration or
input/output records. They SHALL include node identities, run correlation, outcomes, timing, Loop paths, and streaming
invocation metadata.

#### Scenario: Observe finite and streaming execution

- **WHEN** a task, Loop, or streaming workflow executes verbosely
- **THEN** its existing lifecycle boundaries are printed without changing the selected result protocol

### Requirement: Keep local diagnostics independent of export

Local diagnostics SHALL work without exporter configuration and with `--no-telemetry`, using no global provider
installation or export connection. Configured OTLP export SHALL coexist with local diagnostics and retain its existing
buffering and shutdown behavior.

#### Scenario: Diagnose a standalone workflow without a collector

- **WHEN** a standalone runner executes with `-v` and no OTLP endpoint
- **THEN** lifecycle metadata appears on stderr while result stdout and workflow exit behavior remain intact

#### Scenario: Diagnose a telemetry-disabled runner

- **WHEN** a runner compiled with `--no-telemetry` executes with `--verbose`
- **THEN** it prints local lifecycle metadata without requiring the OTLP SDK or opening an export connection

#### Scenario: Combine local diagnostics with export

- **WHEN** a verbose runner has a configured OTLP endpoint and completes successfully or with a handled failure
- **THEN** local diagnostics coexist with correlated export and preserve the workflow outcome

### Requirement: Parse verbosity at runner option boundaries

Runner argument parsing SHALL preserve consumed option-like string values, schema validation, and transport-conflict
rules. `-v`, clusters such as `-vv`, and repeated `--verbose` SHALL accumulate with execution detail capped at three.
CLI counts SHALL accumulate before and after subcommands. CLI options SHALL precede trailing workflow parameters.

#### Scenario: Keep an option-like input value

- **WHEN** `--node.port -vvv` supplies a string value to a declared workflow input
- **THEN** that consumed value remains `-vvv` without increasing verbosity

#### Scenario: Accumulate verbosity

- **WHEN** flags are clustered, repeated, or placed on both sides of a CLI subcommand
- **THEN** their counts are added and any total above three selects level three

#### Scenario: Reject ambiguous option use

- **WHEN** malformed verbosity flags are supplied or named workflow parameters are mixed with JSON transport
- **THEN** parsing fails before node execution

### Requirement: Render readable local lifecycle summaries

At level one, local diagnostics SHALL retain readable progress, outcomes, timing, and port/failure context without
repeated routing fields. Readable Loop paths SHALL use one-based pass numbers and stream identifiers SHALL retain
unsigned precision. Summary lines SHALL show the run ID at workflow start. Level two SHALL add available event
name/sequence, workflow identity, node position, and node kind metadata.

#### Scenario: Scan successful execution

- **WHEN** a workflow executes with `-v`
- **THEN** its existing readable progress and completion summaries remain intact

#### Scenario: Locate a specific execution boundary

- **WHEN** a workflow executes with `-vv`
- **THEN** readable summaries identify the corresponding lifecycle event and available node/workflow metadata

#### Scenario: Correlate local output with export

- **WHEN** readable diagnostics and OTLP export are both enabled
- **THEN** the summary start line identifies the same run without repeating that ID on every readable event line

### Requirement: Keep local diagnostic events on separate lines

Identifiers and failure messages SHALL escape control characters. The local writer SHALL preserve one complete line
per lifecycle event even when execution is concurrent. This presentation SHALL consume the same validated records
used by OTLP and MUST NOT instrument node execution separately or require export SDK support.

#### Scenario: Display an error containing a newline

- **WHEN** a failed node's identifier or error message contains a newline
- **THEN** the newline is escaped within its diagnostic line and cannot become another apparent event

### Requirement: Remind users about long-running nodes

Verbose execution SHALL print a local reminder every 60 seconds while a node invocation remains active, identifying
the node, elapsed running time, and available Loop or stream context. Reminders SHALL use existing lifecycle records,
work without OTLP SDK support, and MUST NOT create synthetic exported events. Quiet execution SHALL start no worker.

#### Scenario: Observe a long-running node without telemetry

- **WHEN** a telemetry-disabled runner executes verbosely and a node remains active beyond 60 seconds
- **THEN** stderr reports that it is still running and repeats at 60-second intervals until completion

### Requirement: Stop reminders for completed or abandoned invocations

Active entries SHALL remain isolated across runs, Loop passes, and stream invocations. Completion, failure, skip,
workflow termination, or observation-scope cleanup SHALL stop the corresponding reminders.

#### Scenario: Complete one of several active invocations

- **WHEN** one invocation completes while another invocation of the same node is still running
- **THEN** reminders stop for the completed invocation and continue only for the active invocation

#### Scenario: Release an abandoned run

- **WHEN** an observation scope is released before workflow completion
- **THEN** its active nodes are removed and no further reminders are written for that run

### Requirement: Display complete lifecycle records at the highest verbosity

Level three SHALL print the complete existing `WireRecord` as compact JSON on stderr, including its attributes, body,
time, and available trace context. It SHALL use the same record passed to OTel logging without changing export or
result stdout. All positive levels SHALL work without the OTLP SDK and retain readable 60-second node reminders.
Compilation SHALL enable its existing build summary at any positive level.

#### Scenario: Inspect a telemetry-disabled runner

- **WHEN** a runner compiled with `--no-telemetry` executes with `-vvv`
- **THEN** lifecycle JSON retains all available fields, result stdout remains intact, and active-node reminders continue

#### Scenario: Correlate the complete record with export

- **WHEN** a level-three runner has configured OTLP export
- **THEN** local records preserve the exported workflow/run identity and available trace correlation

### Requirement: Correlate opt-in snapshots with active spans

Opt-in snapshot LogRecords emitted under a valid active span SHALL preserve its native trace and span IDs. All fragments of one snapshot record SHALL preserve the same association. Existing snapshot bodies, sequence, digest, value references, and workflow/run identities SHALL retain their meanings. Correlation MUST NOT require embedding full business values in span attributes.

#### Scenario: Export node input and output snapshots

- **WHEN** an observed finite node emits opt-in snapshots while its span context is attached
- **THEN** exported snapshot LogRecords identify that node invocation's trace and span

#### Scenario: Fragment a large node record

- **WHEN** a snapshot record exceeds the single-packet limit
- **THEN** every exported fragment retains the record's active trace/span association

#### Scenario: Export without an active span

- **WHEN** a snapshot is emitted without valid trace context
- **THEN** it retains the normal snapshot contract without inventing a trace/span identity
