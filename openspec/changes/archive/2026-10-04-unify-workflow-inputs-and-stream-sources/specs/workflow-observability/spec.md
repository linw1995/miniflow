# Spec Delta

## MODIFIED Requirements

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

### Requirement: Gate unsupported streaming snapshot capture

Streaming instances SHALL reject the current whole-run snapshot recorder before startup execution. Generated
runners MUST reject `MF_CAPTURE_SNAPSHOTS=1` in streaming mode with an actionable diagnostic. Ordinary
bounded-run capture SHALL retain its existing behavior.

#### Scenario: Request stream snapshot history

- **WHEN** snapshot capture is enabled for a streaming runner or attached programmatically to a streaming
  instance
- **THEN** startup rejects the unsupported mode before source admission or node execution

## ADDED Requirements

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
