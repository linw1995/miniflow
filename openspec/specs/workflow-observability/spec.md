# workflow-observability Specification

## Purpose

Expose workflow execution through correlated OpenTelemetry traces and lifecycle events so local and external consumers can observe progress without changing execution behavior.

## Requirements

### Requirement: Identify and correlate workflow observations

Lifecycle events SHALL carry an observation schema version, workflow identity, unique run identity, event timestamp, and
monotonically increasing per-run sequence. Node events SHALL include definition node ID and node kind. A node SHALL have
one execution lifecycle per run, identified by run ID and node ID, without an attempt field. Applicable events SHALL
carry native OTel trace/span correlation. Workflow identity MUST agree with the runner description; separate invocations
MUST have distinct run identities even when they share a distributed trace.

#### Scenario: Observe repeated invocations

- **WHEN** the same executable runs twice
- **THEN** both runs identify the same embedded workflow, use different run IDs, and independently sequence their lifecycle records starting at 1

#### Scenario: Correlate a node event

- **WHEN** an observed node starts execution
- **THEN** its lifecycle event identifies the workflow, run, node, kind, and associated OTel trace/span context without an attempt number

#### Scenario: Preserve a single execution lifecycle

- **WHEN** a node fails or delivery of its telemetry is retried
- **THEN** observation does not schedule another node invocation or introduce a second execution attempt

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

A workflow observation scope SHALL start before preparation and finish after selected-output extraction or a handled
failure. A node start event SHALL mean its dependencies resolved and its implementation is about to be invoked. Node
success MUST follow output validation and publication. Construction and dependency failures SHALL identify their phase
and node when known, without requiring a preceding node start. Generated and in-memory execution MUST produce equivalent
lifecycle meanings and retain existing execution order, skip rules, output values, and error precedence.

#### Scenario: Reject invalid node outputs

- **WHEN** a node returns outputs that fail publication validation
- **THEN** the node and workflow report failure, and no successful node completion is emitted

#### Scenario: Fail during preparation

- **WHEN** node construction fails before node execution begins
- **THEN** the workflow reports preparation failure with the affected node, and no node implementation is reported as having started

#### Scenario: Fail on a missing dependency

- **WHEN** one dependency is unexpectedly absent and another is explicitly skipped
- **THEN** dependency resolution reports node failure before invocation rather than a conditional skip

#### Scenario: Fail during selected-output extraction

- **WHEN** all invoked nodes succeed but a required selected workflow output is skipped
- **THEN** the workflow reports output-selection failure while preserving the successful node outcomes

### Requirement: Distinguish conditional skips from unreached nodes

A node skipped because of resolved conditional dependencies SHALL emit `mf.node.skipped` without a start event or implementation invocation. The event SHALL identify the causal source node and port. Node terminal metadata SHALL expose produced and explicitly skipped port names without business values. Nodes proven unreached by a handled failure's terminal execution boundary SHALL be classified as NotRun rather than Skipped.

#### Scenario: Observe an unselected branch

- **WHEN** a router activates one output and explicitly skips another
- **THEN** its terminal metadata identifies both port outcomes, and a downstream node skipped by the inactive port identifies that dependency without executing

#### Scenario: Stop after an earlier failure

- **WHEN** workflow execution fails before a later node is visited
- **THEN** the final execution boundary identifies that later node as NotRun without claiming it was conditionally skipped

### Requirement: Make lifecycle loss detectable without requiring recovery

Lifecycle sequence numbers SHALL be assigned before serialization or enqueueing so dropped records leave detectable
gaps. A handled run SHALL emit a lightweight `mf.workflow.finished` record whose final sequence equals its own sequence,
with workflow outcome, visited execution prefix, available duration, and failure context. No lifecycle record SHALL be
emitted after this terminal record. Consumers MUST NOT require a complete per-node snapshot or retransmission to observe
a run. Telemetry loss MUST NOT trigger node re-execution or change workflow results.

A consumer SHALL expose visible sequence gaps, known local lifecycle drops, and missing terminal boundaries. It MUST
distinguish detected missing records from an inability to verify the stream's tail. Without a valid terminal boundary it
MUST NOT claim completeness or an exact total missing count, including when the process exits successfully or the entire
telemetry stream is absent. While execution is active, completeness SHALL remain unconfirmed; immediate detection of a
wholly lost suffix without later evidence is not required.

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

### Requirement: Observe Iteration at its outer node boundary

The workflow description and version 1 lifecycle SHALL identify an Iteration as one outer node. Its start and finish
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
