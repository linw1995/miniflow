# Spec Delta

## ADDED Requirements

### Requirement: Declare one bidirectional named-port value contract

Providers SHALL be able to derive one named owned struct contract for both input and output roles. That contract SHALL
expose one port declaration and bidirectional dynamic conversion. A task's input and output roles MUST NOT require
distinct data types or duplicate declarations. Existing directional contracts SHALL remain supported during migration.

#### Scenario: Reuse one struct in both roles

- **WHEN** a provider uses one unified value struct as both its task input and output
- **THEN** both roles expose the same field names, descriptors, and requiredness without a second declaration

#### Scenario: Preserve directional compatibility

- **WHEN** a provider retains an existing input-only or output-only derive or manual contract
- **THEN** its existing typed task remains supported without implementing the opposite conversion direction

#### Scenario: Preserve unified derive identities

- **WHEN** a unified value struct uses generics, type aliases, raw identifiers, exact renames, or an explicit runtime
  dependency path
- **THEN** both conversions and its shared declaration agree on the same port identities and types

### Requirement: Preserve dynamic semantics for unified values

Unified value conversion SHALL retain existing strict scalar and recursive collection representations, optional-port
omission, null data, unknown-field rejection, shared payload identity, and descriptor depth limits. Failures SHALL
preserve their conversion direction, port, typed source chain, and escaped nested pointer. Unifying declarations MUST
NOT introduce coercion or defaults.

#### Scenario: Keep omission distinct from null

- **WHEN** an optional shared field is omitted or explicitly supplied as null
- **THEN** omission remains absent and supplied null remains present data in both conversion directions

#### Scenario: Reject a non-finite output descendant

- **WHEN** a unified value contains a non-finite floating value inside a list or map
- **THEN** encoding fails with the output port and nested typed cause instead of emitting null

#### Scenario: Reject unknown or missing input fields

- **WHEN** decoding receives an unknown port or omits a required field
- **THEN** it fails with the corresponding port error instead of ignoring the value or creating a default

### Requirement: Prove direct typed transfer equivalence

A generated connection SHALL transfer a Rust field directly only when its type identity, presence, resolved port
compatibility, and codec validation equivalence are proven. Equal JSON descriptors or matching type names alone MUST NOT
establish that proof. Any value-dependent constraints SHALL be checked before the corresponding producer publication or
consumer invocation.

#### Scenario: Transfer a compatible owned field

- **WHEN** a required owned string output has one consumer with the same Rust field type and proven compatible contracts
- **THEN** the generated connection transfers that field without an intermediate dynamic map or value encode/decode
  round trip

#### Scenario: Retain custom validation

- **WHEN** producer and consumer fields share a Rust type but the consumer decoder has an unproven business constraint
- **THEN** generated execution retains dynamic decoding rather than bypassing that constraint

#### Scenario: Reject a different Rust representation

- **WHEN** two fields declare the same JSON category but have distinct Rust types
- **THEN** the compiler does not infer a direct assignment or an implicit conversion between them

#### Scenario: Validate every producer field before transfer

- **WHEN** a producer returns a valid connected field and an invalid unused floating field
- **THEN** execution fails at the producer before the consumer runs and before any result becomes available

#### Scenario: Preserve a refined shared-value check

- **WHEN** a shared payload violates its prepared refined output descriptor
- **THEN** typed generation either performs the equivalent check before publication or retains the dynamic path
