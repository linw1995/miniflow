## Purpose

Allow one node invocation to return an authoritative Task result while optionally delivering incremental stream values with explicit consumption and lifetime semantics.

## ADDED Requirements

### Requirement: Execute terminal and stream capabilities together

A combined node SHALL run one business invocation with both incremental publication and a terminal result available. It MUST NOT require a per-node Task/Stream mode selection or independently invoke two business entry points. Terminal results SHALL follow ordinary validation, context commit, skip, and success-dependency rules.

#### Scenario: Return a final result after live values

- **WHEN** a combined node emits several increments and returns a terminal result
- **THEN** consumers can receive increments before return and terminal dependents receive the validated final result once
- **AND** the node's business operation executes once

#### Scenario: Emit no values

- **WHEN** a connected combined node returns success without emitting
- **THEN** its stream invocation closes normally and its terminal result remains available
- **AND** no empty value, skipped message, or consumer Task invocation is invented

### Requirement: Declare separate output categories

Combined nodes SHALL declare separate terminal and stream result contracts with distinct port names. Type checking and runtime validation SHALL preserve each port's delivery category. Required emission fields constrain each emitted record and MUST NOT imply that an invocation emits at least one record.

#### Scenario: Reject a missing field in an actual emission

- **WHEN** an emitted record omits a required stream field
- **THEN** publication fails with that field and node identity even if terminal output is otherwise valid

#### Scenario: Keep terminal fields out of emission validation

- **WHEN** a node declares terminal text and stream delta fields
- **THEN** an emission requires only its stream contract and the final result requires only its terminal contract

### Requirement: Discard streams without consumers

An unconnected stream SHALL be valid without an explicit discard node and SHALL reserve no pending-delivery capacity. Nodes SHALL be able to detect consumer absence and omit emission entirely. Every attempted emission SHALL retain encoding, output validation, cancellation, and publication-error semantics before discard.

#### Scenario: Ignore live output and wait for completion

- **WHEN** a host executes a combined node without graph stream consumers or a live subscription
- **THEN** the node completes with its terminal result without an unread output queue

#### Scenario: Skip emission construction

- **WHEN** a node detects no consumers and constructs no incremental values
- **THEN** execution remains valid and its terminal operation is not skipped

#### Scenario: Attempt invalid discarded output

- **WHEN** a node attempts to emit a malformed value into an unconnected stream
- **THEN** validation fails rather than silently accepting the invalid output

### Requirement: Preserve connected delivery obligations

Connected streams SHALL deliver accepted values in order through bounded capacity and apply backpressure. Unexpected receiver closure or publication failure MUST fail the invocation and wake blocked senders rather than silently switching to discard. A successful send denotes admission, not downstream completion.

#### Scenario: Lose a subscribed receiver

- **WHEN** a connected receiver closes before its invocation is finalized
- **THEN** the invocation receives a delivery failure and cannot claim successful live delivery

#### Scenario: Ignore a publication error

- **WHEN** business code ignores a failed emission and returns a successful terminal result
- **THEN** the runtime still reports the publication failure and suppresses successful terminal publication

### Requirement: Identify and close each stream invocation

Subscribed combined-node exports SHALL publish explicit open and close controls even with zero data records. Controls and values SHALL identify the export, execution, unique node invocation, and dynamic scope path. Closing one invocation MUST NOT close later invocations of the same node. A node that is never invoked SHALL publish no invocation controls.

#### Scenario: Repeat a node in a loop

- **WHEN** the same static node executes in two loop iterations
- **THEN** each execution has a separate stream identity and close boundary

#### Scenario: Skip an unselected branch

- **WHEN** a branch containing an exported node is not activated
- **THEN** the runtime does not invent a stream invocation for that node

### Requirement: Keep terminal results authoritative

Terminal results SHALL be returned directly rather than reconstructed from emitted values. Execution failure SHALL suppress successful terminal publication while preserving already delivered increments as provisional data. Final workflow success SHALL wait for connected delivery and downstream drain.

#### Scenario: Postprocess streamed text

- **WHEN** a workflow transforms the generated text after the live stream closes
- **THEN** its final selected text is authoritative even when it differs from the live prefix

#### Scenario: Fail after a delivered prefix

- **WHEN** a combined invocation returns an execution error after several delivered emissions
- **THEN** those values remain identifiable as provisional and no successful terminal result is published
