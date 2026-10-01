# Spec Delta

## MODIFIED Requirements

### Requirement: Identify and correlate workflow observations

Lifecycle events SHALL carry an observation schema version, workflow identity, unique run identity,
event timestamp, and monotonically increasing per-run sequence. Node events SHALL include a local
definition node ID, kind, and structured Loop path with pass indices. A node invocation SHALL have
one lifecycle identified by run ID, Loop path, and local node ID; repeated passes SHALL be separate
invocations, not retry attempts. Top-level nodes SHALL use an empty Loop path. Applicable events
SHALL carry native OTel trace/span correlation. Workflow identity MUST agree with the runner
description; separate invocations MUST have distinct run identities even when they share a
distributed trace. Existing protocol versions SHALL keep their prior single-invocation
interpretation.

#### Scenario: Observe repeated invocations

- **WHEN** the same executable runs twice
- **THEN** both runs identify the same embedded workflow, use different run IDs, and independently sequence their lifecycle records starting at 1

#### Scenario: Correlate a node event

- **WHEN** an observed node starts execution
- **THEN** its lifecycle event identifies the workflow, run, node, kind, path, and associated OTel trace/span context without an attempt number

#### Scenario: Preserve a single execution lifecycle

- **WHEN** a node invocation fails or delivery of its telemetry is retried
- **THEN** observation does not schedule another invocation or introduce a retry attempt

#### Scenario: Correlate repeated body invocations

- **WHEN** a Loop body node runs in two passes
- **THEN** its events share workflow and run identity but have different pass paths and independent lifecycle states

#### Scenario: Keep transport duplicates distinct from repeated work

- **WHEN** an event is redelivered and the body node also runs in another pass
- **THEN** the redelivery is deduplicated by sequence while the later pass remains a separate invocation

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

## ADDED Requirements

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
