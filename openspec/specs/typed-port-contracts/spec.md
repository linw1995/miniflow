# typed-port-contracts Specification

## Purpose

Give all workflow nodes one shared language for precise JSON port types and enforce those declarations consistently during graph validation and execution.

## Requirements

### Requirement: Describe precise and legacy JSON port types

Port descriptors SHALL retain `Any`, `Null`, `Boolean`, `Number`, `String`, `Array`, and `Object` and add `Int64`,
`Float64`, `List(T)`, and `Map(T)` for recursively described element or value type `T`. `Map(T)` SHALL describe a JSON
object with string keys and values of type `T`; `List(T)` SHALL describe a JSON array whose elements have type `T`.
`Int64` SHALL require a signed 64-bit JSON integer, and `Float64` SHALL require a finite floating JSON number. Legacy
`Number`, `Array`, and `Object` SHALL remain broad categories. Plugins SHALL be able to expose different refined types
for different node instances.

#### Scenario: Describe a nested collection

- **WHEN** a node declares `List(Map(Int64))`
- **THEN** its port contract denotes an array of JSON objects whose values are signed 64-bit integers

#### Scenario: Preserve legacy descriptors

- **WHEN** an existing plugin uses constructor-based `Number`, `Array`, or `Object` ports
- **THEN** its port names and broad JSON categories remain available after rebuilding against the updated runtime

### Requirement: Classify graph connections by type evidence

Compiler validation SHALL accept a statically safe connection when every value admitted by the source type is admitted
by the target type. It SHALL accept a dynamically checked connection when a broad or `Any` component at a compatible
structural position requires the runtime to check the actual value before the target executes. It MUST reject disjoint
concrete types and incompatible collection shapes before installation, even though an empty collection could satisfy two
different element descriptors. `Any` SHALL be a dynamic boundary, not proof of a concrete type. Recursive list and map
compatibility SHALL follow the same rules for element or value types. No numeric, string, or collection coercion SHALL
occur.

#### Scenario: Connect a refined value to a broad input

- **WHEN** an `Int64` output feeds a `Number` input, or `List(Int64)` feeds an `Array` input
- **THEN** compilation accepts a statically safe connection

#### Scenario: Guard a dynamic source

- **WHEN** an `Any` output feeds an `Int64` input, or an `Array` output feeds a `List(Int64)` input
- **THEN** compilation accepts the connection with a runtime type check at the target

#### Scenario: Guard a nested broad element

- **WHEN** a `List(Number)` output feeds a `List(Int64)` input
- **THEN** compilation accepts the connection and recursively checks actual elements at execution

#### Scenario: Reject a disjoint connection

- **WHEN** a `String` output feeds an `Int64` input, a `Float64` output feeds an `Int64` input, or a `List(String)` output feeds a `List(Int64)` input
- **THEN** compilation rejects the edge with both endpoints and expected types

### Requirement: Enforce port types before execution and publication

The runtime SHALL validate every produced value against its declared output type before publishing any result from that
node. For an active node, it SHALL validate each bound input against its declared input type before invoking the node.
Validation SHALL recurse through lists and maps and report the node, port, expected type, actual type, and failing JSON
Pointer path. A violation MUST fail the workflow without coercion or partial publication. Output values produced under
`Any` SHALL remain unrestricted. Explicit skips MUST retain their existing behavior: a skipped node is not type-checked,
while an unexpectedly missing dependency remains an error even when another dependency is skipped.

#### Scenario: Reject a lying producer

- **WHEN** a plugin declares output `Int64` but returns a JSON string, including on a port with no consumer
- **THEN** execution fails before publishing any output from that node

#### Scenario: Reject an invalid dynamic binding

- **WHEN** a produced `Any` value is a string and feeds an `Int64` input
- **THEN** execution fails before invoking the consumer and identifies its input port

#### Scenario: Report a nested mismatch

- **WHEN** a `List(Map(Int64))` input receives `[ {"count": 1}, {"count": "two"} ]`
- **THEN** execution fails with the path `/1/count` and the expected integer type

#### Scenario: Preserve skip and missing precedence

- **WHEN** a target has one explicitly skipped dependency and another unexpectedly missing dependency
- **THEN** the missing-output error is reported before any type check or target invocation

#### Scenario: Preserve null as data

- **WHEN** a `Null` output produces JSON null or an `Any` output produces JSON null
- **THEN** the value is published as present data rather than a skip marker

### Requirement: Share validation across execution paths

In-memory Flow execution and generated binaries SHALL apply the same refined output checks, dynamic-boundary input checks, diagnostics, and skip precedence. The generated runner MUST NOT need node-kind-specific type guards. Existing node implementations using static broad port constructors and ordinary `execute` methods SHALL remain valid when their returned values satisfy their declarations.

#### Scenario: Match generated and in-memory failures

- **WHEN** the same workflow passes a wrong nested JSON value across a dynamically checked edge
- **THEN** both execution paths fail at the same consumer with equivalent type and path diagnostics

#### Scenario: Keep an ordinary plugin

- **WHEN** a plugin retains its static `PortSpec::new` declarations and produces values matching them
- **THEN** it runs without adding a type-specific execution method
