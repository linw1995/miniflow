# task-stream-node-execution Specification

## Purpose

Allow one node invocation to return an authoritative Task result while optionally delivering incremental stream values with explicit consumption and lifetime semantics.

## Requirements

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

### Requirement: Validate port declarations in the compiler

The compiler SHALL reject empty or duplicate port names and names shared across terminal and incremental
outputs through its node metadata validation before execution. Node preparation SHALL reflect those
declarations without executing the node. Invalid declarations MUST NOT reach business execution.

#### Scenario: Reject an ambiguous output category

- **WHEN** terminal and incremental contracts use the same port name
- **THEN** compilation fails with the conflicting node and port identities before any executor runs

#### Scenario: Reject duplicate or empty incremental names

- **WHEN** a provider reflects duplicate or empty incremental port names
- **THEN** compilation rejects the declarations through the same metadata validation as ordinary ports

### Requirement: Validate every attempted emission

The emitter SHALL encode typed results and validate each emitted result against its incremental contract.
Required fields SHALL constrain each emitted record rather than require an invocation to emit. Encoding,
validation, and delivery failures SHALL be returned through ordinary `Result` values with their typed
source chains. Providers MAY propagate or handle recoverable errors. Cancellation SHALL be checked
before and after business execution and on attempted publication.

#### Scenario: Propagate a send failure

- **WHEN** a provider propagates an encoding or delivery error with `?`
- **THEN** combined execution fails with the original typed source and publishes no terminal result

#### Scenario: Handle a recoverable send failure

- **WHEN** a provider handles a failed emission before returning a valid terminal result
- **THEN** the emitter does not poison that result or prevent subsequent valid emissions
- **AND** cancellation and an already failed stream instance still stop execution

#### Scenario: Emit an invalid discarded record

- **WHEN** an unconnected provider attempts to emit a value with missing or incompatible fields
- **THEN** emission validation fails despite the absence of a consumer

### Requirement: Reuse shared output error contracts

Combined execution SHALL reuse ordinary typed-output conversion errors, workflow output validation
errors, and stream delivery errors. The emitter MUST NOT introduce emission-specific error categories,
shared-error carriers, or remembered-error state.

#### Scenario: Preserve one output failure across node kinds

- **WHEN** ordinary Task publication and an emission violate the same output contract
- **THEN** both report the shared workflow output error and preserve that error through failure propagation

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

#### Scenario: Handle a live delivery failure

- **WHEN** business code handles a failed publication to a subscribed live receiver
- **THEN** live delivery cancellation still prevents successful invocation completion

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

### Requirement: Keep live delivery independent of byte quotas

Runtime live channels SHALL bound pending delivery by record count. Live channels and the finite live protocol MUST NOT impose pending-byte, per-record, final-result, or cumulative byte quotas. Protocol validation, ordered delivery, cancellation, and receiver-disconnection behavior SHALL remain enforced.

#### Scenario: Deliver a large live record

- **WHEN** a valid emitted value exceeds the previous default byte limits
- **THEN** runtime live delivery accepts it subject to pending record capacity

#### Scenario: Continue after a large cumulative output

- **WHEN** consumers drain valid live records whose combined wire size exceeds the previous cumulative byte limit
- **THEN** publication and final-result decoding continue without a runtime byte-budget failure
