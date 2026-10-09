## MODIFIED Requirements

### Requirement: Return complete prepared nodes

Node factories SHALL return a prepared node containing metadata reflected from the executor's input and output
contracts and its complete execution implementation. Metadata SHALL include ports, output derivations, and declared
context references. The compiler SHALL validate and resolve this metadata before execution. Execution traits MUST NOT
require hooks that complete a partially constructed node. Reflected input contracts and resource conditions MUST remain
stable between generated-build and runtime preparation for the same configuration and selected provider, including
across host and target implementations. Runtime resource initialization MAY fail without changing these contracts.

#### Scenario: Prepare dynamic ports

- **WHEN** a provider prepares a checked program or validated dynamic schema
- **THEN** its factory reflects ports from that execution contract and compilation validates them without executing the node

#### Scenario: Preserve configured declarations across environments

- **WHEN** the same configured provider is prepared during generated compilation and runner startup in different process environments
- **THEN** it reports the same input contracts and resource conditions while retaining independent executor state

#### Scenario: Preserve declarations across host and target builds

- **WHEN** host and target builds use platform-specific executor initialization
- **THEN** their reflected input contracts and resource conditions agree

#### Scenario: Keep initialization failures separate

- **WHEN** a provider cannot initialize an executor because a required runtime resource is unavailable
- **THEN** preparation reports the construction failure without substituting a different input contract

### Requirement: Integrate typed inputs with existing preparation consumers

Typed task metadata SHALL participate in compiler validation, startup interfaces, manifest comparison, and generated execution through the existing prepared-node contract. Derived declarations MUST remain independent of invocation values and process state. Dynamic task, event, and stream providers SHALL remain supported through reflected contracts without adopting typed execution.

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

- **WHEN** a dynamic provider validates configuration into its execution input contract
- **THEN** it reflects that contract through `NodePortContract` and retains map execution without an input struct

### Requirement: Refine typed output descriptors without competing declarations

Typed tasks MAY narrow reflected outputs using separate output evidence. Evidence MUST reference existing ports,
preserve declared names and required flags, and admit only values allowed by the reflected descriptor. Invalid or
duplicate evidence MUST fail compiler preparation before business execution. Proven types MUST respect the shared
descriptor-depth limit. Actual produced values SHALL retain ordinary runtime publication checks.

#### Scenario: Refine a shared list from a prepared body

- **WHEN** a typed task collects shared values from a body declaring signed integer results
- **THEN** reflection declares the shared-list field and separate evidence resolves it to `List(Int64)`

#### Scenario: Reject competing output declarations

- **WHEN** a factory supplies an output port list alongside its reflected value contract
- **THEN** construction fails even when the supplied names, requiredness, and descriptors are identical

#### Scenario: Reject an invalid output refinement

- **WHEN** evidence references an unknown output, duplicates an output derivation, widens its descriptor, or exceeds the depth limit
- **THEN** compiler preparation fails without invoking business logic

### Requirement: Prepare tasks from unified value contracts

Typed preparation SHALL accept unified named-port values in either task role and derive their ports from the canonical
declaration. It SHALL preserve other metadata, reject competing declarations, and retain typed construction errors.
Output type evidence SHALL be separate from reflection and validated by ordinary compiler inference. Preparation MUST
NOT invoke business logic or convert invocation values.

#### Scenario: Prepare the same value contract in both roles

- **WHEN** a task uses the same unified struct for input and output and provides forwarding metadata
- **THEN** preparation derives both port directions and retains forwarding without decoding, encoding, or executing the task

#### Scenario: Preserve output refinement validation

- **WHEN** a unified output contract has a type proven by configuration or a prepared body
- **THEN** reflection retains the canonical declaration and compiler inference validates and resolves the separate evidence

## ADDED Requirements

### Requirement: Reflect contracts across executor kinds

Task, event, and stream constructors SHALL reflect both port directions from `NodePortContract`. Fixed interfaces
SHALL use `NodePorts::from_types`; dynamic interfaces SHALL reflect validated execution contracts. Constructors MUST
reject factory port declarations, preserve other metadata, and avoid invoking execution methods. Low-level assembly
of an existing `NodeExecution` and metadata MAY remain available separately.

#### Scenario: Reflect fixed event and stream bags

- **WHEN** an event or stream provider has fixed named input and output value types
- **THEN** construction reflects both directions through the shared contract interface without an extra typed execution trait

#### Scenario: Reflect a checked dynamic program

- **WHEN** a CEL provider has validated input conversion types and checked output programs
- **THEN** reflected ports match those same contracts used to bind inputs and encode results

#### Scenario: Reject duplicate sources of truth

- **WHEN** a task, event, or stream factory supplies input or output port metadata beside its executor's contract
- **THEN** construction fails before execution even if the metadata matches the reflected contract

#### Scenario: Preserve preparation-only metadata

- **WHEN** a reflected provider declares conditional stdin ownership, context references, or output evidence
- **THEN** construction preserves those fields without reading inputs, dispatching events, or running business logic
