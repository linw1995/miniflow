# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: Prepare typed tasks from one input definition`
- TO: `### Requirement: Prepare typed tasks from input and output definitions`

## MODIFIED Requirements

### Requirement: Prepare typed tasks from input and output definitions

Typed task preparation SHALL derive both port directions from owned structs and preserve derivations, context references, and resource requirements. Separate input or output declarations MUST fail with a typed construction error, including identical declarations. Preparation MUST NOT execute business logic, encode values, or require invocation arguments.

#### Scenario: Prepare a typed task without arguments

- **WHEN** a factory prepares a typed task with a required string and an optional shared JSON field before invocation values exist
- **THEN** its metadata contains the derived input and output ports without invoking the task or converting invocation values

#### Scenario: Reject competing declarations

- **WHEN** a caller supplies input or output port metadata to typed preparation, even if it matches the corresponding struct
- **THEN** preparation returns a typed construction error instead of overriding either declaration

#### Scenario: Preserve provider metadata

- **WHEN** a typed identity provider declares output forwarding and a typed provider declares context references or conditional stdin ownership
- **THEN** preparation retains those declarations for ordinary compiler and runtime validation

### Requirement: Adapt typed task execution within the runtime

The runtime SHALL decode resolved inputs, invoke typed business logic, and encode its output struct as ordinary task results. Explicit skipped names and loop summaries SHALL be preserved. Encoding MUST complete before publication. Existing threading, dependency resolution, skip precedence, input checks, and output checks SHALL retain their execution boundaries.

#### Scenario: Invoke typed business logic

- **WHEN** an active typed task receives inputs satisfying its declared struct
- **THEN** runtime conversion supplies the input struct, business logic executes once with the existing mutable context, and the output struct is encoded

#### Scenario: Skip without decoding

- **WHEN** existing dependency rules skip a typed task
- **THEN** neither struct conversion nor typed business logic executes

#### Scenario: Preserve missing dependency precedence

- **WHEN** a typed task has one skipped dependency and another unexpectedly missing output
- **THEN** the missing dependency error is reported before struct decoding or business execution

#### Scenario: Validate typed task outputs

- **WHEN** typed business logic returns an output that contradicts its prepared declaration
- **THEN** execution fails before publishing that task's result through the same output checks used by dynamic tasks

#### Scenario: Preserve typed execution control metadata

- **WHEN** a typed result emits explicit skips, present null data, or a loop summary
- **THEN** adaptation preserves all three independently of output value encoding

#### Scenario: Reject encoding before publication

- **WHEN** one typed output encodes successfully but a later output cannot be encoded
- **THEN** workflow execution fails with the producer and typed cause and publishes none of that result

## ADDED Requirements

### Requirement: Refine typed output descriptors without competing declarations

Typed tasks SHALL be able to narrow output descriptors using fixed configuration or a prepared body. Refinements MUST preserve every declared name and required flag, contain no duplicate ports, and admit only values allowed by the struct descriptor. Invalid refinements MUST fail during preparation. Actual produced values SHALL remain subject to existing output publication checks.

#### Scenario: Refine a shared list from a prepared body

- **WHEN** a typed task collects shared values from a body declaring signed integer results
- **THEN** its prepared output can be List(Int64) while its owned field remains a list of shared JSON values

#### Scenario: Reject an invalid output refinement

- **WHEN** a refinement omits, duplicates, renames, changes presence, widens, or contradicts a struct output
- **THEN** preparation fails without invoking business logic

### Requirement: Integrate typed outputs through existing execution consumers

Struct-defined outputs SHALL participate in ordinary compiler connection validation and in-memory and generated task execution, including stream task domains. Dynamic task providers SHALL retain their map result interface. Typed providers SHALL adopt owned result declarations without requiring workflow definition changes or node-specific compiler guards.

#### Scenario: Reject a disjoint derived output connection

- **WHEN** a derived integer output connects to a concrete string input
- **THEN** compilation rejects the connection before execution

#### Scenario: Match generated and in-memory typed outputs

- **WHEN** a typed task executes in-memory or in a generated synchronous or streaming runner
- **THEN** both paths encode equivalent JSON and reject invalid floating outputs before publication
