## ADDED Requirements

### Requirement: Distinguish dynamic invocation observations

Description version 2026-10-28 SHALL support invocation path entries identifying caller, binding,
selected target, and invocation count. Repeated calls SHALL have distinct paths. Receivers SHALL
validate route identity and nested caller continuity, and SHALL reject target events for another node.
Invocation counts MUST NOT use Loop pass limits. Existing static scope wire shapes SHALL remain valid.

#### Scenario: Observe repeated calls independently

- **WHEN** one caller invokes the same target more than once
- **THEN** lifecycle and snapshot paths retain distinct invocation identities
- **AND** finite and stream receivers do not overwrite the target's root-node state

#### Scenario: Reuse physical scope templates

- **WHEN** a target invokes another target or enters an ordinary nested Flow
- **THEN** description lookup follows physical scopes and reusable invocation templates
- **AND** templates preserve physical node/dependency contracts without inflating executor counts

#### Scenario: Reject forged call provenance

- **WHEN** an event names an undeclared binding/target pair or a nested caller different from the active target
- **THEN** receiver validation rejects the event
- **AND** reordered or duplicate valid events retain their ordinary delivery semantics

#### Scenario: Inspect a dynamic caller in the TUI

- **WHEN** a caller with an invocation template is selected
- **THEN** body navigation can inspect the invocation and show its selected target and binding
- **AND** the caller retains its real node kind rather than being relabeled as a static Flow call

### Requirement: Parse versioned directed-call declarations

Workflow version 2026-10-28 SHALL accept scope-local `calls` maps indexed by caller ID and alias. Each
declaration SHALL name an exact target, explicit port mappings, and an opt-in remaining-input policy.
Older versions MUST reject the field. Empty caller groups, unknown endpoints, duplicate destination
mappings, recursive call graphs, and ordinary outgoing uses of call targets MUST fail planning.

#### Scenario: Preserve declaration semantics through normalization

- **WHEN** a call declaration combines explicit ports with remaining-input fallback
- **THEN** canonical serialization preserves both policies and their scope-local identities
- **AND** omitting the fallback policy retains explicit-only binding

#### Scenario: Preserve nested declarations without activating them

- **WHEN** a Loop or Iteration body contains a version-supported call declaration
- **THEN** definition normalization retains the declaration in that body
- **AND** executable preparation rejects it until invocation scheduling supports the scope

### Requirement: Freeze and verify private call layouts

The compiler SHALL emit deterministic versioned call layouts containing workflow identity, canonical
node identities, and resolved bindings. Saved layouts SHALL be checked against freshly prepared
contracts before executable use. Public discovery MUST project interfaces rather than serialize the
private plan. Parsing planning evidence alone MUST NOT grant an execution capability.

#### Scenario: Detect layout or definition drift

- **WHEN** a saved layout differs from a freshly prepared definition or binding contract
- **THEN** verification fails rather than dispatching using stale indexes or parameter ownership

#### Scenario: Fail closed before invocation scheduling is available

- **WHEN** executable preparation or runner generation encounters directed-call declarations
- **THEN** the intermediate implementation reports an unsupported execution error before business work
- **AND** ordinary definitions without call declarations retain their execution behavior

### Requirement: Preserve the exact caller parameter constraints

A caller-visible interface SHALL retain local binding identity and alias with separate output and
default namespaces. Shared outputs SHALL appear once with conjunctive source/destination constraints,
OR-combined requiredness and sensitivity, and deterministic ordering. External inputs, omitted inputs,
target indexes, dependency keys, and configuration values MUST NOT be retained in the public view.

#### Scenario: Preserve a nullable intersection

- **WHEN** one caller output feeds both a nullable string and a nullable integer input
- **THEN** the interface requires both constraints and accepts null as their shared value
- **AND** it does not widen the parameter to Any or treat the types as alternatives

#### Scenario: Keep argument namespaces distinct

- **WHEN** a caller output and an unrelated default input share a name
- **THEN** they remain separate parameters in separate namespaces
- **AND** public validation agrees with resolved argument binding on acceptance

#### Scenario: Serialize a public view safely

- **WHEN** a resolved binding has external credentials and internal prerequisite nodes
- **THEN** serialized and debug representations of its public interface omit those records
- **AND** the caller-owned parameter types and sensitivity remain available

### Requirement: Bind discovery to the prepared execution scope

A prepared Flow or stream SHALL accept an explicit immutable interface catalog for its task callers.
Execution SHALL expose only the active caller's interfaces and override any enclosing graph's catalog,
including with an empty catalog. Scope exit SHALL restore enclosing metadata and node identity on
success, failure, and unwinding. Discovery attachment MUST NOT activate targets or alter scheduling.

#### Scenario: Reject ambiguous catalogs

- **WHEN** a catalog repeats a caller or a caller-local binding ID or alias
- **THEN** construction fails before attachment
- **AND** attachment rejects unknown or non-task owners

#### Scenario: Isolate same-named nodes in nested flows

- **WHEN** nested prepared flows contain the same node definition ID
- **THEN** each node sees only its own Flow's catalog
- **AND** a child without a catalog sees no interfaces and the enclosing view is restored afterward

#### Scenario: Preserve discovery across execution paths

- **WHEN** a task executes dynamically, through a generated domain, via snapshot fallback, or in isolation
- **THEN** it observes the same owning scope's interfaces
- **AND** combined producers on subsequent stream messages retain their scope's catalog

#### Scenario: Preserve fork boundaries

- **WHEN** a context forks a domain or stream frame
- **THEN** immutable catalog storage is shared without carrying an active caller identity
- **AND** a body fork starts without an inherited catalog

#### Scenario: Prepare multiple callers together

- **WHEN** different callers bind different partially supplied targets in one scope
- **THEN** catalog preparation validates the union of target declarations through one prepared scope
- **AND** unrelated required inputs retain ordinary validation

### Requirement: Declare call outputs independently

Providers SHALL declare explicit call output slots separately from ordinary and stream outputs.
Preparation SHALL reject overlapping namespaces, invalid names, and excessive type nesting. Ordinary
result publication and output references MUST NOT treat call slots as terminal or stream outputs.

#### Scenario: Preserve ordinary output validation

- **WHEN** a task declares required call slots but produces only its ordinary terminal result
- **THEN** ordinary execution does not require values for the unused call slots
- **AND** ordinary data edges cannot reference those call slots

#### Scenario: Detect call output drift

- **WHEN** a provider changes a nonempty call-output contract after generated metadata was frozen
- **THEN** the shared metadata comparison rejects the change
- **AND** providers without call slots retain their previous serialized metadata shape

### Requirement: Resolve calls through prepared node contracts

Compiler call resolution SHALL reuse normal node preparation, type compatibility, and exclusive-branch
proofs. Only declared call targets in the current root scope MAY defer input completeness validation
until caller mappings and external bindings have been resolved.

#### Scenario: Resolve a partially bound target

- **WHEN** a target receives a token through ordinary data connections and query through a caller mapping
- **THEN** call preparation resolves the query binding after preparing the token producer's inferred type
- **AND** ordinary workflow preparation still rejects the same missing query without call declarations

#### Scenario: Keep nested scope validation strict

- **WHEN** preparing a call scope containing a nested Flow or Loop
- **THEN** root call-target declarations do not waive input requirements inside that nested body

### Requirement: Validate caller arguments against resolved ownership

A resolved binding SHALL validate supplied names against its explicit-output and default-input
namespaces, then check requiredness and both source and destination value contracts. A successful
binding SHALL return caller-owned values under target input names, preserving sharing and sensitivity.
External and omitted inputs MUST NOT be inserted, read, or overridden by this operation. A failed
binding MUST NOT expose a partial result or execute a node.

#### Scenario: Preserve source and target constraints

- **WHEN** a broad source feeds a narrow target, or a narrow source feeds a broad target
- **THEN** argument binding validates the value against both declarations using runtime type rules
- **AND** failures retain typed causes and nested value paths

#### Scenario: Scope required outputs to the selected binding

- **WHEN** a Parent declares outputs for multiple callable bindings
- **THEN** a selected binding requires only its own mapped outputs and defaults
- **AND** a shared mapped output must satisfy every destination in that binding

#### Scenario: Preserve control dependencies without claiming parameters

- **WHEN** a target has output-control or node-success prerequisites
- **THEN** resolution retains those dependencies and their producer indexes
- **AND** they do not remove target inputs from the caller's default ownership set

#### Scenario: Reject unimplemented invocation resources

- **WHEN** a target requires an external context reference, stdin, or startup environment/default binding
- **THEN** the initial resolver rejects it before execution instead of dropping the requirement
- **AND** input sensitivity declarations remain supported

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
