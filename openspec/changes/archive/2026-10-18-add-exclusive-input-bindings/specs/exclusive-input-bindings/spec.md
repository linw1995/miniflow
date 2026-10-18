## Purpose

Bind one node input to explicitly declared alternative outputs while preserving deterministic execution, ordinary activation gates, and value metadata.

## ADDED Requirements

### Requirement: Declare an exclusive input group

Workflow, loop body, and iteration body definitions SHALL accept an optional `exclusive_inputs` array, defaulting to empty. Each entry SHALL contain `to_node`, `to_input`, and `sources`; each source SHALL contain `from_node` and `from_output`. A group MUST name at least two distinct candidate outputs and one existing input. A target input MUST have exactly one binding declaration across ordinary edges and exclusive groups.

#### Scenario: Bind alternative requests

- **WHEN** `git.diff.request` declares `local.value` and `ci.value` as exclusive sources
- **THEN** both outputs supply alternatives for that one input without requiring a merge node

#### Scenario: Reject malformed or competing bindings

- **WHEN** a group has fewer than two sources, duplicate sources, unknown fields or endpoints, or a target already bound by an edge or another group
- **THEN** compilation rejects the declaration with the target and offending source or field

#### Scenario: Preserve ordinary duplicate rejection

- **WHEN** two ordinary edges bind the same target input without an exclusive declaration
- **THEN** compilation rejects the duplicate binding rather than inferring exclusivity

### Requirement: Validate every candidate dependency

Every candidate SHALL participate in dependency ordering, cycle detection, scope validation, context-reference ancestry, and input connectivity checks. A valid exclusive group SHALL satisfy its target's connected-input requirement. All candidates MUST belong to the consumer's input message domain and scope; existing scope-source bindings SHALL remain supported.

#### Scenario: Reject a cycle through an alternative

- **WHEN** one candidate creates a dependency cycle even though that branch is expected to be skipped
- **THEN** compilation rejects the cycle before execution

#### Scenario: Reject cross-domain selection

- **WHEN** candidates combine a pre-stream output with a stream message, or outputs from independent message domains
- **THEN** compilation rejects the group instead of buffering or correlating messages

### Requirement: Resolve the unique produced candidate

The core SHALL resolve all candidates for the current frame before invoking the consumer. Exactly one produced candidate SHALL bind its value to the target input. All explicitly skipped candidates SHALL resolve the input as skipped. More than one produced candidate MUST fail with the target input and active source identities. Candidate declaration and completion order MUST NOT choose the winner.

#### Scenario: Select one branch

- **WHEN** one candidate produces a value, every other candidate explicitly skips, and the other consumer dependencies are ready
- **THEN** the consumer executes once with that value at the declared input

#### Scenario: Propagate an all-skipped group

- **WHEN** every candidate explicitly skips
- **THEN** the input resolves as skipped and follows the consumer's existing skip behavior, including connected optional inputs and collection boundaries

#### Scenario: Detect a later competing result

- **WHEN** one candidate produces a value before another candidate resolves and that other candidate later also produces a value
- **THEN** the consumer has not executed and resolution fails, even when both values are equal

#### Scenario: Preserve false and null

- **WHEN** the unique produced candidate is false or null and the target type accepts it
- **THEN** the value binds normally rather than being treated as absent

### Requirement: Preserve dependency failures before skips

Unexpectedly missing candidate outputs MUST fail rather than count as skipped. Candidate absence and ambiguity MUST NOT be hidden by another skipped input or control gate. Upstream execution errors and cancellation SHALL retain existing termination behavior and MUST NOT select a fallback candidate. A pending producer MUST NOT be classified as skipped.

#### Scenario: Reject missing data behind a skip

- **WHEN** one candidate is unexpectedly absent and another dependency explicitly skips
- **THEN** dependency resolution fails without invoking the consumer, regardless of declaration order

#### Scenario: Reject ambiguity behind a control gate

- **WHEN** two candidates produce values and a separate control dependency skips
- **THEN** dependency resolution reports the ambiguous input rather than silently skipping it

#### Scenario: Preserve failure and cancellation

- **WHEN** an unresolved candidate fails or the workflow is cancelled after another candidate produces a value
- **THEN** the consumer is not invoked using that partial selection and existing termination behavior applies

### Requirement: Validate candidate types conservatively

Each candidate SHALL independently satisfy existing static or runtime-checked compatibility rules for the target input, including known-value checks. Inference MUST use evidence valid for every possible selected candidate and MUST NOT derive a type or exact value from one arbitrary source. For an active consumer, the selected value SHALL pass existing input validation before invocation.

#### Scenario: Reject an incompatible unselected branch

- **WHEN** one candidate has a statically incompatible type even though a router is expected to skip it
- **THEN** compilation rejects that candidate

#### Scenario: Retain a dynamic boundary

- **WHEN** an unknown Any candidate is uniquely selected for an Int64 input and produces a string
- **THEN** execution fails with the existing typed input error before invoking the consumer

#### Scenario: Avoid arbitrary forwarded evidence

- **WHEN** an identity input selects between different constants or different compatible source types
- **THEN** downstream inference does not adopt a single candidate's constant or an unjustified narrow type

### Requirement: Preserve selected value metadata

Selection SHALL retain the selected shared payload and its sensitivity metadata without serializing or reconstructing it. Business nodes SHALL receive their existing single-value input contract. Separate exclusive input groups SHALL resolve independently; correlated fields MUST be carried together in one value when callers require selection from one branch.

#### Scenario: Forward a secret structured request

- **WHEN** the unique produced candidate is a shared object with secret fields
- **THEN** the consumer receives the same shared payload and field sensitivity, and existing output redaction still applies

#### Scenario: Resolve separate input groups

- **WHEN** two input groups each have a unique produced candidate from different branches
- **THEN** both resolve independently without an implied cross-input branch constraint

### Requirement: Keep execution paths and frames equivalent

In-memory and generated execution SHALL implement the same selection, error, skip, and metadata behavior for Tasks and supported Event and Stream consumers. Supported loop and iteration bodies SHALL resolve groups within each invocation. Typed optimization MUST preserve these semantics or use the ordinary shared-value path with an explicit fallback reason.

#### Scenario: Match compiled and in-memory outcomes

- **WHEN** the same workflow exercises unique, all-skipped, ambiguous, missing, or wrong-type candidates
- **THEN** both execution paths produce equivalent results and diagnostics

#### Scenario: Isolate repeated frames and body invocations

- **WHEN** successive frames or body invocations select different candidates or skip all candidates
- **THEN** each resolution uses only its own outcomes and never reuses a previously selected value

#### Scenario: Preserve collection behavior

- **WHEN** a collector receives an all-skipped exclusive input while holding prior accepted values
- **THEN** no item is added and the existing buffer and deadline remain unchanged

### Requirement: Describe exclusive sources without exposing values

Workflow descriptions SHALL distinguish candidate groups from mandatory inputs and control edges. When execution observation is enabled, it SHALL identify the selected source for a resolved group; dependency failures SHALL identify the target and relevant sources without exposing payloads. Selection MUST NOT appear as a user-authored merge node.

#### Scenario: Inspect a selected branch

- **WHEN** an observed consumer selects `ci.value` for input `request`
- **THEN** its input-resolution observation identifies that source and target without copying the request into diagnostics

### Requirement: Preserve existing definition behavior

Omitting `exclusive_inputs` SHALL preserve existing behavior, and empty groups arrays SHALL be omitted on serialization. Using the new field SHALL require updated compiler and runtime support; older readers MUST reject unsupported syntax rather than reinterpret it. Isolated node execution SHALL continue to accept resolved inputs without running or selecting workflow predecessors.

#### Scenario: Run an unchanged workflow

- **WHEN** a workflow omits `exclusive_inputs`
- **THEN** its bindings, activation, skips, and results retain their existing semantics

#### Scenario: Execute a consumer in isolation

- **WHEN** a caller directly executes a node whose workflow input normally has exclusive candidates
- **THEN** the caller supplies the resolved input through the existing isolated-input contract and no candidate producer executes
