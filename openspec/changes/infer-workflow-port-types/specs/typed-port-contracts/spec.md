# Spec Delta

## MODIFIED Requirements

### Requirement: Classify graph connections by type evidence

Compiler validation SHALL accept a statically safe connection when every value admitted by the source type is
admitted by the target type. When the source has an exact known value, it SHALL instead validate that value
against the target type and MUST reject a mismatch before installation, even if a broad inferred source
descriptor would otherwise allow a dynamically checked connection. A compatible exact value SHALL be accepted,
including an empty collection that satisfies a refined collection input. Without an exact value, compiler
validation SHALL accept a dynamically checked connection when a broad or `Any` component at a compatible
structural position requires the runtime to check the actual value before the target executes. It MUST reject
disjoint concrete types and incompatible collection shapes before installation, even though an unknown output
could happen to produce an empty collection satisfying both descriptors. `Any` SHALL remain a dynamic boundary
when no exact value is known, not proof of a concrete type. Recursive list and map compatibility SHALL follow
the same rules for element or value types. No numeric, string, or collection coercion SHALL occur.

#### Scenario: Connect a refined value to a broad input

- **WHEN** an `Int64` output feeds a `Number` input, or `List(Int64)` feeds an `Array` input
- **THEN** compilation accepts a statically safe connection

#### Scenario: Guard a dynamic source

- **WHEN** an unknown `Any` output feeds an `Int64` input, or an unknown `Array` output feeds a `List(Int64)` input
- **THEN** compilation accepts the connection with a runtime type check at the target

#### Scenario: Guard a nested broad element

- **WHEN** an unknown `List(Number)` output feeds a `List(Int64)` input
- **THEN** compilation accepts the connection and recursively checks actual elements at execution

#### Scenario: Reject a disjoint connection

- **WHEN** a `String` output feeds an `Int64` input, a `Float64` output feeds an `Int64` input, or an unknown `List(String)` output feeds a `List(Int64)` input
- **THEN** compilation rejects the edge with both endpoints and their types

#### Scenario: Reject a known heterogeneous mismatch

- **WHEN** a known `[1, "x"]` output feeds a `List(Int64)` input
- **THEN** compilation rejects the edge and identifies the mismatch at `/1` instead of deferring it to execution

#### Scenario: Accept a known empty collection

- **WHEN** a known empty array feeds a `List(Int64)` input
- **THEN** compilation accepts the edge because the exact value satisfies the target type

#### Scenario: Reject a known null mismatch

- **WHEN** a known null value feeds an `Int64` input
- **THEN** compilation rejects the edge before installation

## ADDED Requirements

### Requirement: Expose optional output derivation metadata

Nodes MAY declare that an output is an exact configured JSON value or an unchanged copy of a named input.
Validation SHALL use such declarations only for the node's existing ports and MUST reject malformed derivation
metadata or an exact value that contradicts its own output declaration, with node and port context. Nodes that
provide no derivation metadata SHALL retain their declared or configuration-derived port types and existing
runtime boundary checks. Execution SHALL continue to validate actual produced and bound values against the
resolved ports.

#### Scenario: Preserve an ordinary plugin

- **WHEN** a plugin declares an `Any` output and supplies no output derivation
- **THEN** a connection to an `Int64` input remains runtime-checked

#### Scenario: Reject an invalid derivation

- **WHEN** a plugin derives an output from a nonexistent input port
- **THEN** runner validation fails before installation and identifies the plugin node and invalid port reference

#### Scenario: Reject a contradictory literal declaration

- **WHEN** a plugin declares a `String` output but derives it from an exact integer literal
- **THEN** runner validation rejects the node metadata before installation

#### Scenario: Guard a false derivation

- **WHEN** a plugin declares an output derivation but produces a value outside its resolved output type
- **THEN** execution fails before publishing that output
