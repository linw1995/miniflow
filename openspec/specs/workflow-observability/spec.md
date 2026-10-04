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
execution spans. Description and validation modes MUST NOT emit workflow execution events.

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

- **WHEN** a runner containing an Iteration node is invoked with `--describe`
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
