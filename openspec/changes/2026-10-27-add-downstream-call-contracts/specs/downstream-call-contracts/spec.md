## ADDED Requirements

### Requirement: Model call outputs separately from ordinary publication

The runtime SHALL provide an explicit call-binding declaration with a target, an atomic group of
source-output-to-target-input mappings, and an opt-in node-level default policy. A call request SHALL
be a separate contract from ordinary task results and stream emissions. Request presence, rather than
argument presence, SHALL select a binding. Declaring these contracts MUST NOT enable node scheduling.

#### Scenario: Declare an explicit group without implicit defaults

- **WHEN** a caller constructs a binding using the default input policy
- **THEN** the group uses explicit port mappings only
- **AND** the declaration does not claim any other target inputs

#### Scenario: Reject duplicate destinations within a group

- **WHEN** two mappings in one caller group name the same target input
- **THEN** declaration construction rejects the ambiguous group
- **AND** mapping one source output to two distinct target inputs remains valid

#### Scenario: Preserve separate argument namespaces

- **WHEN** an explicit source output and an unrelated default target input have the same name
- **THEN** their values coexist in separate argument namespaces within one request
- **AND** shared payload identity, sensitivity, and omission semantics are preserved

#### Scenario: Select a zero-argument target

- **WHEN** a request explicitly selects a binding with an empty argument group
- **THEN** the envelope represents one call rather than no call
- **AND** it does not publish an ordinary output or skip any other binding

### Requirement: Correlate structured call outcomes

A call identity SHALL include its caller-local binding and a workflow-execution-unique invocation
number allocated by the dispatcher. A response SHALL retain that identity and SHALL support checking
it against an expected request before yielding its outcome. Outcomes SHALL distinguish successful
outputs, dependency skips, and typed runtime failures without encoding errors as strings or propagating
loop scope effects in the response.

#### Scenario: Complete repeated calls independently

- **WHEN** two requests select the same binding with different invocation numbers
- **THEN** their responses can be correlated independently, including in reverse completion order

#### Scenario: Reject a mismatched response

- **WHEN** a response is checked against a different binding or invocation number
- **THEN** outcome extraction fails with both the actual and expected identity

#### Scenario: Retain structured failure and skip causes

- **WHEN** a response contains a runtime failure or dependency skip
- **THEN** the original typed error chain or structured dependency causes remain accessible

### Requirement: Resolve port bindings before invocation defaults

The compiler SHALL assign target input ownership from explicit port bindings first. A node-level
invocation connection SHALL assign only remaining unbound target inputs to its Parent. Control and
success dependencies MUST NOT assign input ownership. Binding precedence SHALL be determined before
execution and MUST NOT depend on value availability.

#### Scenario: Bind all inputs by default

- **WHEN** Parent has a node-level invocation connection to a target without explicit input bindings
- **THEN** Parent supplies every target input, preserving each input's requiredness

#### Scenario: Compose explicit external inputs with defaults

- **WHEN** a constant and a configuration service explicitly bind two target input ports
- **AND** Parent has a node-level invocation connection to that target
- **THEN** those two ports belong to their explicit producers
- **AND** Parent supplies only the remaining inputs

#### Scenario: Limit an explicit caller to its connected ports

- **WHEN** a callable binding maps only Parent's query port to the target query input
- **AND** no node-level default invocation connection is declared
- **THEN** Parent owns only query, and unrelated target inputs are not implicitly assigned to Parent

#### Scenario: Reject an unbound required input

- **WHEN** explicit-only bindings leave a required target input without a source
- **THEN** preparation fails with the target and missing input identified

#### Scenario: Preserve an unbound optional input

- **WHEN** explicit-only bindings leave an optional target input unbound
- **THEN** that input remains omitted and is not added to Parent's parameter interface

#### Scenario: Keep control edges out of parameter resolution

- **WHEN** a node has only a control or success connection to a target
- **THEN** that connection assigns no target input and creates no callable capability

#### Scenario: Reject conflicting explicit sources

- **WHEN** multiple explicit sources bind one target input without a valid exclusive binding
- **THEN** preparation fails instead of choosing a source by connection order

#### Scenario: Reject mixed caller and external exclusive candidates

- **WHEN** an exclusive group combines caller invocation arguments with an external producer
- **THEN** callable preparation rejects the unsupported mixed selection contract

### Requirement: Expose only the resolved public interface

The runtime SHALL provide deterministic, read-only connection views within the current prepared
scope. A caller's parameter interface SHALL contain exactly the target inputs assigned to that caller,
using final resolved types, requiredness, and public descriptions. Views MUST NOT expose private
configuration, literal derivation values, or executor handles.

#### Scenario: Exclude externally supplied credentials

- **WHEN** token is explicitly bound to a configuration service
- **THEN** token is absent from Parent's callable parameter schema
- **AND** the service value is not included in discovery metadata

#### Scenario: Discover before target execution

- **WHEN** Parent inspects a prepared downstream interface before the target runs
- **THEN** discovery succeeds without executing the target or reading its outputs

#### Scenario: Restore the enclosing view

- **WHEN** a nested invocation finishes and control returns to Parent
- **THEN** Parent sees its enclosing scope's connection view, not the child's local node indexes

#### Scenario: Preserve generated execution parity

- **WHEN** the same workflow is executed dynamically or through a generated runner
- **THEN** the caller observes the same ordered bindings and resolved public contracts
- **AND** startup rejects relevant drift from the compiled contract

### Requirement: Preserve ordinary dataflow activation

Ordinary data connections SHALL retain existing fan-out and dependency behavior. Callable invocation
SHALL require an explicit callable declaration. Its port mappings SHALL be grouped atomically without
implicitly enabling node-level default binding. Initial callable targets SHALL use on-demand terminal
Task execution and MUST NOT also participate in ordinary DAG activation as the same instance.

#### Scenario: Leave ordinary fan-out unchanged

- **WHEN** a source output has multiple ordinary data consumers
- **THEN** existing dependency rules continue to determine all eligible consumer executions

#### Scenario: Do not execute an unselected tool

- **WHEN** Parent completes without invoking an available tool
- **THEN** the tool does not run and its absence does not prevent workflow completion

#### Scenario: Reject mixed activation

- **WHEN** one target instance is declared for both ordinary activation and callable activation
- **THEN** preparation rejects the unsupported mixed lifecycle

### Requirement: Compose inputs within an isolated invocation

The runtime SHALL accept caller arguments atomically and compose them with the binding's external
inputs before target execution. Every invocation SHALL have an independent identity and result scope.
Unknown arguments and externally owned argument names MUST be rejected. External-source failure or
skip MUST NOT reassign input ownership to Parent.

#### Scenario: Reject an external input override

- **WHEN** Parent supplies token although token belongs to an external port binding
- **THEN** the invocation fails validation before executing the target

#### Scenario: Keep concurrent arguments separate

- **WHEN** two invocations supply different query and limit pairs
- **THEN** each target invocation receives its own pair without mixing arguments across invocations

#### Scenario: Preserve absence and null

- **WHEN** Parent omits an optional input or explicitly supplies null
- **THEN** omission remains absence and null is accepted only if permitted by the declared type

#### Scenario: Do not fall back after an external skip

- **WHEN** required external input resolution skips a tool invocation
- **THEN** the runtime reports the skipped outcome to Parent without executing the target
- **AND** the runtime does not request or accept that input from Parent as a fallback

#### Scenario: Return results to the originating invocation

- **WHEN** a tool finishes
- **THEN** its validated result returns to the originating caller invocation
- **AND** it does not mark the reusable target complete in the parent DAG or activate ordinary successors

### Requirement: Bound invocation dependencies and execution

Preparation SHALL establish external prerequisites without introducing waits on caller completion.
Invocation execution SHALL preserve runtime validation, observation, cancellation, deadlines, shared
worker limits, and execution budgets. The initial implementation SHALL serialize calls to each
prepared target instance and reject recursive callable dependencies.

#### Scenario: Reject a caller completion cycle

- **WHEN** a tool's external input producer can run only after its waiting caller completes
- **THEN** preparation rejects the dependency cycle

#### Scenario: Execute with one worker

- **WHEN** Parent invokes a tool with a shared worker limit of one
- **THEN** execution makes progress without requiring a second worker held behind the waiting Parent

#### Scenario: Refresh configuration per invocation

- **WHEN** configuration refresh is placed inside a callable Flow
- **THEN** each invocation evaluates that configuration within its own child scope
- **AND** external configuration producers remain governed by ordinary graph scheduling

#### Scenario: Propagate cancellation

- **WHEN** the caller is cancelled while a tool invocation is active
- **THEN** the child observes the inherited cancellation and cannot escape the caller's execution budget
