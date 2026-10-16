## ADDED Requirements

### Requirement: Persist only manifest source facts

Manifest `2026-10-16` SHALL store the compiled definition, root and nested Loop execution orders, and prepared input
schema. It MUST NOT persist derived description, interface, or workflow ID properties. The input schema SHALL retain
provider-declared types, bindings, and stdin constraints absent from definition JSON. Full static configuration is
included; invocation values MUST NOT be materialized. Existing framing and size bounds SHALL remain in force.

#### Scenario: Serialize a current manifest

- **WHEN** the compiler serializes a manifest from one provider preparation
- **THEN** the payload contains the source facts without duplicated graph, interface, or identity views

#### Scenario: Preserve provider input facts

- **WHEN** a provider declares a typed input and conditional stdin requirement
- **THEN** manifest helpers retain those declarations without loading the provider during inspection

### Requirement: Derive manifest views without replanning

Runtime helpers SHALL derive graph, interface, and identity views without replanning or invoking providers. They SHALL
borrow nodes by exact root ID and expose definition and input schema directly. Readers SHALL accept only the current
manifest layout. Invalid orders, inconsistent input contracts, missing facts, and unexpected derived properties MUST
fail validation.

#### Scenario: Read original node configuration

- **WHEN** a caller looks up a node containing configuration expressions and nested values
- **THEN** it receives the original kind and configuration without resolving expressions or executing a factory

#### Scenario: Preserve frozen execution order

- **WHEN** compiled root or nested Loop order differs from definition storage order
- **THEN** derived node positions follow the compiled order without recalculating it

#### Scenario: Preserve observations and portable inspection

- **WHEN** generated task, stream, and nested runners use manifest helpers after relocation or release/LTO/stripping
- **THEN** configuration remains readable without execution and results and observation identities remain unchanged

#### Scenario: Derive the event protocol

- **WHEN** a reader derives an execution description from a manifest
- **THEN** its event schema is computed from description version and execution mode without a stored event-version field

## MODIFIED Requirements

### Requirement: Embed a versioned workflow manifest

New generated executables SHALL contain one versioned manifest with the compiled definition, execution orders, and complete startup input schema. Graph and interface views MUST derive the same workflow identity. The manifest MUST be available without source files, sidecars, a symbol table, or executing the runner. It SHALL include static node configuration, exclude invocation values, and bound its combined JSON payload to 16 MiB.

#### Scenario: Inspect a moved executable

- **WHEN** a newly compiled executable is moved without source, plugins, a toolchain, or generated build files
- **THEN** its graph, observation compatibility, startup input declarations, and stdin conditions remain readable from that executable alone

#### Scenario: Preserve metadata exclusions

- **WHEN** a node has static configuration and a workflow invocation supplies business values
- **THEN** the original configuration is present in the manifest and invocation values are absent

#### Scenario: Describe a newly compiled older schema

- **WHEN** a supported definition predating startup-input promotion is compiled by the new compiler
- **THEN** its executable has a manifest retaining its existing graph protocol and empty startup interface without introducing promoted inputs
