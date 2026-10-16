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

### Requirement: Derive versioned manifest views without replanning

Runtime helpers SHALL derive graph, interface, and identity views from frozen facts using versioned rules, without
planning execution or invoking providers. They SHALL borrow original nodes by exact root ID. Readers SHALL preserve
legacy `2026-10-07` views and return `None` for unavailable definitions. Invalid root/Loop orders, inconsistent input
contracts, missing source facts, and unexpected derived properties MUST fail on encoding or decoding as applicable.

#### Scenario: Read original node configuration

- **WHEN** a caller looks up a node containing configuration expressions and nested values
- **THEN** it receives the original kind and configuration without resolving expressions or executing a factory

#### Scenario: Preserve frozen execution order

- **WHEN** compiled root or nested Loop order differs from definition storage order
- **THEN** derived node positions follow the compiled order without recalculating it

#### Scenario: Preserve legacy inspection

- **WHEN** a reader opens a supported legacy manifest without original definitions
- **THEN** graph and interface helpers retain its existing views and node-definition lookup returns `None`

#### Scenario: Preserve observations and portable inspection

- **WHEN** generated task, stream, and nested runners use manifest helpers after relocation or release/LTO/stripping
- **THEN** configuration remains readable without execution and results and observation identities remain unchanged

#### Scenario: Preserve versioned protocol meaning

- **WHEN** a reader derives views for a startup-aware workflow in manifest `2026-10-16`
- **THEN** single and stream views retain event schemas 2 and 4 respectively, independent of future runtime protocols
