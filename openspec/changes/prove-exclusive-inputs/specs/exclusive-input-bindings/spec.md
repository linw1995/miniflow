## ADDED Requirements

### Requirement: Prove exclusive candidates before execution

Compilation MUST prove every pair of candidates mutually exclusive using declared task output exclusivity and graph activation dependencies. Unknown or unproven exclusivity MUST be a compile error identifying the target input and candidate pair. Programmatic executable construction MUST enforce the same rule. Structural-only planning MAY defer node metadata validation until preparation, but MUST NOT produce an executable without proof.

#### Scenario: Prove router branches

- **WHEN** candidate producers depend transitively on different outputs of the same declared exclusive router
- **THEN** compilation accepts the group without evaluating runtime conditions

#### Scenario: Reject unproven alternatives

- **WHEN** candidates have no branch constraints, share one branch, or depend on unrelated routers
- **THEN** compilation rejects the group before any workflow node executes

#### Scenario: Preserve only common facts after convergence

- **WHEN** an exclusive input joins different branch outputs
- **THEN** its consumer inherits only facts common to every alternative, and cannot claim either individual branch as a downstream proof

#### Scenario: Reject invalid output contracts

- **WHEN** exclusive output metadata contains unknown ports, duplicate ports, a single port, or belongs to a stateful Event or Stream node
- **THEN** preparation rejects the node contract

#### Scenario: Keep facts local to an invocation

- **WHEN** candidate proofs involve a loop body, iteration body, or stream frame
- **THEN** compilation uses facts within that scope and does not propagate per-invocation facts through stateful boundaries

## MODIFIED Requirements

### Requirement: Resolve the unique produced candidate

The core SHALL resolve all candidates for the current frame before invoking the consumer. Exactly one produced candidate SHALL bind its value to the target input. All explicitly skipped candidates SHALL resolve the input as skipped. Executable groups MUST have compile-time mutual-exclusion proof. If a node implementation or unchecked plan violates that contract, resolution MUST fail closed as an internal contract error. Candidate declaration and completion order MUST NOT choose the winner.

#### Scenario: Select one branch

- **WHEN** one candidate produces a value, every other candidate explicitly skips, and the other consumer dependencies are ready
- **THEN** the consumer executes once with that value at the declared input

#### Scenario: Propagate an all-skipped group

- **WHEN** every candidate explicitly skips
- **THEN** the input resolves as skipped and follows the consumer's existing skip behavior, including connected optional inputs and collection boundaries

#### Scenario: Defend against a violated node contract

- **WHEN** a node implementation violates its declared exclusive output contract and multiple candidates produce values
- **THEN** resolution fails closed before the consumer executes, without exposing payloads

#### Scenario: Preserve false and null

- **WHEN** the unique produced candidate is false or null and the target type accepts it
- **THEN** the value binds normally rather than being treated as absent

### Requirement: Preserve dependency failures before skips

Unexpectedly missing candidate outputs MUST fail rather than count as skipped. Candidate absence MUST NOT be hidden by another skipped input or control gate. Upstream execution errors and cancellation SHALL retain existing termination behavior and MUST NOT select a fallback candidate. A pending producer MUST NOT be classified as skipped.

#### Scenario: Reject missing data behind a skip

- **WHEN** one candidate is unexpectedly absent and another dependency explicitly skips
- **THEN** dependency resolution fails without invoking the consumer, regardless of declaration order

#### Scenario: Reject unproven groups behind a control gate

- **WHEN** candidate mutual exclusion cannot be proven and the consumer has a separate control gate
- **THEN** compilation rejects the group regardless of whether that gate might skip

#### Scenario: Preserve failure and cancellation

- **WHEN** an unresolved candidate fails or the workflow is cancelled after another candidate produces a value
- **THEN** the consumer is not invoked using that partial selection and existing termination behavior applies

### Requirement: Keep execution paths and frames equivalent

In-memory and generated execution SHALL implement the same selection, error, skip, and metadata behavior for Tasks and supported Event and Stream consumers. Supported loop and iteration bodies SHALL resolve groups within each invocation. Typed optimization MUST preserve these semantics or use the ordinary shared-value path with an explicit fallback reason.

#### Scenario: Match compiled and in-memory outcomes

- **WHEN** the same workflow exercises unique, all-skipped, missing, or wrong-type candidates
- **THEN** both execution paths produce equivalent results and diagnostics

#### Scenario: Isolate repeated frames and body invocations

- **WHEN** successive frames or body invocations select different candidates or skip all candidates
- **THEN** each resolution uses only its own outcomes and never reuses a previously selected value

#### Scenario: Preserve collection behavior

- **WHEN** a collector receives an all-skipped exclusive input while holding prior accepted values
- **THEN** no item is added and the existing buffer and deadline remain unchanged
