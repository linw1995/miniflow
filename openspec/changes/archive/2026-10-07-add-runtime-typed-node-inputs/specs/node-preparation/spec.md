# Spec Delta

## ADDED Requirements

### Requirement: Prepare typed tasks from one input definition

Typed task preparation SHALL derive all input port names, types, and required flags from the task's input struct. It SHALL preserve caller-supplied output declarations, derivations, context references, and resource requirements. Supplying separate input declarations to typed preparation MUST fail with a construction error. Preparation MUST NOT execute business logic or require invocation values.

#### Scenario: Prepare a typed task without arguments

- **WHEN** a factory prepares a typed task with a required string and an optional shared JSON field before invocation values exist
- **THEN** its metadata contains a required string port and a non-required Any port without invoking the task

#### Scenario: Reject competing declarations

- **WHEN** a caller supplies input port metadata to typed preparation, even if it matches the input struct
- **THEN** preparation returns a typed construction error instead of overriding either declaration

#### Scenario: Preserve provider metadata

- **WHEN** a typed identity provider declares output forwarding and a typed provider declares context references or conditional stdin ownership
- **THEN** preparation retains those declarations for ordinary compiler and runtime validation

### Requirement: Adapt typed task execution within the runtime

The runtime SHALL decode resolved inputs into the declared struct and invoke typed business logic without provider-written map conversion. Typed tasks SHALL retain the existing task threading and execution contract. Existing dependency resolution, skip precedence, bound-input checks, and output publication checks SHALL apply before or after typed execution at their current boundaries.

#### Scenario: Invoke typed business logic

- **WHEN** an active typed task receives inputs satisfying its declared struct
- **THEN** runtime conversion supplies the struct and business logic executes once with the existing mutable execution context

#### Scenario: Skip without decoding

- **WHEN** existing dependency rules skip a typed task
- **THEN** neither struct conversion nor typed business logic executes

#### Scenario: Preserve missing dependency precedence

- **WHEN** a typed task has one skipped dependency and another unexpectedly missing output
- **THEN** the missing dependency error is reported before struct decoding or business execution

#### Scenario: Validate typed task outputs

- **WHEN** typed business logic returns an output that contradicts its prepared declaration
- **THEN** execution fails before publishing that task's result through the same output checks used by dynamic tasks

### Requirement: Integrate typed inputs with existing preparation consumers

Typed task metadata SHALL participate in compiler validation, startup interfaces, manifest comparison, and generated execution through the existing prepared-node contract. Derived declarations MUST remain independent of invocation values and process state. Legacy dynamic task, event, and stream providers SHALL remain supported without adopting typed inputs.

#### Scenario: Reject an incompatible typed edge

- **WHEN** a concrete string output is connected to a derived integer input
- **THEN** ordinary compiler validation rejects the connection before execution

#### Scenario: Expose typed startup parameters

- **WHEN** an initial typed task has required, optional, or renamed struct fields
- **THEN** its workflow interface exposes the derived port names, descriptors, and required flags using existing argument validation rules

#### Scenario: Freeze generated typed declarations

- **WHEN** a generated runner prepares a typed task whose startup declarations differ from its embedded interface
- **THEN** existing manifest comparison rejects launch before business execution or source consumption

#### Scenario: Use typed tasks in both execution modes

- **WHEN** a typed task executes in a synchronous flow or a stream message domain
- **THEN** the same prepared adapter supplies typed inputs and preserves the corresponding execution context

#### Scenario: Retain configuration-dependent inputs

- **WHEN** a dynamic provider derives its input ports from configuration
- **THEN** its existing preparation and execution APIs continue to operate without an input struct
