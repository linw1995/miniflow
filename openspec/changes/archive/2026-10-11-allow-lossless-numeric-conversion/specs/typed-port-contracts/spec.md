# Spec Delta

## MODIFIED Requirements

### Requirement: Describe precise and legacy JSON port types

Port descriptors SHALL retain `Any`, `Null`, `Boolean`, `Number`, `String`, `Array`, and `Object` and add `Int64`,
`Float64`, `List(T)`, and `Map(T)` for recursively described element or value type `T`. `Map(T)` SHALL describe a JSON
object with string keys and values of type `T`; `List(T)` SHALL describe a JSON array whose elements have type `T`.
`Int64` SHALL require a number exactly representable as a signed 64-bit integer, and `Float64` SHALL require a finite
number exactly representable as a double. Legacy
`Number`, `Array`, and `Object` SHALL remain broad categories. Plugins SHALL be able to expose different refined types
for different node instances.

#### Scenario: Describe a nested collection

- **WHEN** a node declares `List(Map(Int64))`
- **THEN** its port contract denotes an array of JSON objects whose values are signed 64-bit integers

#### Scenario: Preserve legacy descriptors

- **WHEN** an existing plugin uses constructor-based `Number`, `Array`, or `Object` ports
- **THEN** its port names and broad JSON categories remain available after rebuilding against the updated runtime

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
the same rules for element or value types. Overlapping numeric ranges SHALL retain runtime checks when narrowing is unproven. Implicit numeric conversion SHALL preserve value, precision, and signed zero. String or collection coercion SHALL NOT occur.

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

- **WHEN** a `String` output feeds an `Int64` input, or an unknown `List(String)` output feeds a `List(Int64)` input
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

### Requirement: Decode typed inputs without changing JSON semantics

Typed input decoding SHALL reject missing required fields, unknown fields, and values outside the declared descriptor without lossy conversion or implicit defaults. Missing optional fields SHALL decode as None; supplied values SHALL decode as Some of the decoded inner value. Explicit null SHALL remain supplied data subject to the inner descriptor. Recursive validation SHALL honor existing type depth limits.

#### Scenario: Preserve optional omission

- **WHEN** a caller omits an optional string field
- **THEN** typed decoding returns None for that field

#### Scenario: Reject optional scalar null

- **WHEN** a caller explicitly supplies null to an optional string field
- **THEN** validation rejects the value rather than decoding it as None

#### Scenario: Retain optional raw null

- **WHEN** a caller explicitly supplies null to an optional ValueRef field
- **THEN** typed decoding returns Some containing a shared null value

#### Scenario: Require mandatory fields

- **WHEN** typed decoding receives a map lacking a required field
- **THEN** decoding fails with the missing port name instead of creating a default value

#### Scenario: Reject unknown fields

- **WHEN** typed decoding receives a name outside the input struct's declared ports
- **THEN** decoding fails with the unknown port name

#### Scenario: Preserve strict numeric representations

- **WHEN** a numeric field receives an integer or floating value exactly representable in its target type
- **THEN** decoding preserves the numeric value through an implicit conversion, rejecting any precision or range loss

#### Scenario: Bound recursive declarations

- **WHEN** a derived collection descriptor exceeds the shared type depth limit
- **THEN** preparation validation rejects it with the existing typed depth error

### Requirement: Preserve dynamic semantics for unified values

Unified value conversion SHALL retain existing strict scalar and recursive collection representations, optional-port
omission, null data, unknown-field rejection, shared payload identity, and descriptor depth limits. Failures SHALL
preserve their conversion direction, port, typed source chain, and escaped nested pointer. Numeric conversions SHALL be exact; nonnumeric coercion and implicit defaults MUST NOT occur. Explicit field defaults SHALL apply only to absent bindings.

#### Scenario: Keep omission distinct from null

- **WHEN** an optional shared field is omitted or explicitly supplied as null
- **THEN** omission remains absent and supplied null remains present data in both conversion directions

#### Scenario: Reject a non-finite output descendant

- **WHEN** a unified value contains a non-finite floating value inside a list or map
- **THEN** encoding fails with the output port and nested typed cause instead of emitting null

#### Scenario: Reject unknown or missing input fields

- **WHEN** decoding receives an unknown port or omits a required field
- **THEN** it fails with the corresponding port error instead of ignoring the value or creating an undeclared default

### Requirement: Check unsigned and single-precision numeric codecs

Unsigned 64-bit, target-sized unsigned, and single-precision fields SHALL expose uint, usize, and float descriptors.
Numeric decoding SHALL accept integer or floating values only when exactly representable in the target type. It MUST
reject fractions for integer targets, overflow, precision loss, and negative-zero-to-integer conversions. Encoding and certification MUST reject
non-finite floats, preserve integer values, and retain negative zero. Recursive errors SHALL retain their pointers.

#### Scenario: Preserve numeric limits

- **WHEN** values contain unsigned limits, target-sized limits, finite single-precision limits, or negative zero
- **THEN** their codecs retain the integer value or single-precision bits through a round trip

#### Scenario: Reject incompatible representations and ranges

- **WHEN** an unsigned field receives a negative or fractional number, or a single-precision field receives a nonrepresentable value
- **THEN** decoding fails with the declared numeric descriptor and failing path

#### Scenario: Reject non-finite output values

- **WHEN** a single-precision field or collection member contains infinity or NaN
- **THEN** encoding and certified validation fail rather than replacing the number with null

## ADDED Requirements

### Requirement: Validate exact numeric boundaries without saturation

Numeric codecs and descriptors SHALL agree on exact representability. Integer-to-float conversion SHALL preserve all
significant binary bits, including exact large powers of two. Float-to-integer conversion SHALL require finite integral
values within the target's half-open range and reject negative zero. Narrowing to single precision SHALL preserve the
stored floating bits through a round trip. Descriptor checks SHALL enforce the same conditions before node invocation.

#### Scenario: Accept exact cross-representation input

- **WHEN** a float field receives integer 42 or an integer field receives floating 42.0
- **THEN** decoding succeeds and preserves the numeric value

#### Scenario: Reject precision loss while accepting large exact values

- **WHEN** single or double precision receives an integer above its precision limit
- **THEN** exact powers of two remain valid while values with too many significant bits fail

#### Scenario: Reject casts that would saturate

- **WHEN** an unsigned maximum integer is converted to a float or a floating value equals an integer target's exclusive upper bound
- **THEN** decoding rejects the value instead of accepting a saturated round-trip result

#### Scenario: Preserve negative zero

- **WHEN** a negative floating zero is decoded into a floating or integer target
- **THEN** floating decoding preserves its sign and integer decoding rejects the sign-losing conversion

#### Scenario: Preserve generated numeric normalization

- **WHEN** a generated typed chain receives exact integer-to-float or integral-float-to-integer inputs
- **THEN** it returns the same normalized values and typed failures as in-memory execution
