# Spec Delta

## MODIFIED Requirements

### Requirement: Derive typed input declarations from owned fields

Struct-defined inputs SHALL map bool, i64, f64, String, and ValueRef to Boolean, Int64, Float64, String, and Any ports. Vec and string-keyed BTreeMap fields SHALL recursively describe lists and maps. Ordinary fields without explicit defaults SHALL be required; a top-level Option field SHALL retain its inner descriptor and be non-required. Declaration and decoding semantics SHALL agree, including through type aliases.

#### Scenario: Describe a typed collection

- **WHEN** an input struct contains a field of type Vec of BTreeMap of String to i64
- **THEN** that field exposes a required List(Map(Int64)) port

#### Scenario: Describe an optional typed field

- **WHEN** an input struct contains an Option of String field
- **THEN** that field exposes a non-required String port rather than Any or a nullable descriptor

#### Scenario: Describe a raw shared payload

- **WHEN** an input struct contains a ValueRef field
- **THEN** that field exposes a required Any port

#### Scenario: Preserve type aliases

- **WHEN** a field uses an alias of Option of String or an alias of a supported collection type
- **THEN** its declaration and decoded value have the same semantics as the underlying type

### Requirement: Derive typed output declarations from owned fields

Struct-defined outputs SHALL map bool, i64, f64, String, and shared JSON values to Boolean, Int64, Float64, String, and Any ports. Lists and string-keyed maps SHALL recursively describe their values. Ordinary fields without explicit unified-contract defaults SHALL be required; top-level optional fields SHALL retain the inner descriptor and be non-required. Type aliases SHALL preserve these semantics.

#### Scenario: Declare recursive output values

- **WHEN** an output struct contains a list of string-keyed integer maps
- **THEN** its port is a required List(Map(Int64)) and encoded values satisfy that descriptor

#### Scenario: Declare optional output values

- **WHEN** an output struct contains an optional string field or a supported type alias
- **THEN** its port is non-required String with the same encoding semantics as the underlying type

### Requirement: Preserve dynamic semantics for unified values

Unified value conversion SHALL retain existing strict scalar and recursive collection representations, optional-port
omission, null data, unknown-field rejection, shared payload identity, and descriptor depth limits. Failures SHALL
preserve their conversion direction, port, typed source chain, and escaped nested pointer. Unifying declarations MUST
NOT introduce implicit coercion or implicit defaults. Explicit field defaults SHALL apply only to absent bindings.

#### Scenario: Keep omission distinct from null

- **WHEN** an optional shared field is omitted or explicitly supplied as null
- **THEN** omission remains absent and supplied null remains present data in both conversion directions

#### Scenario: Reject a non-finite output descendant

- **WHEN** a unified value contains a non-finite floating value inside a list or map
- **THEN** encoding fails with the output port and nested typed cause instead of emitting null

#### Scenario: Reject unknown or missing input fields

- **WHEN** decoding receives an unknown port or omits a required field
- **THEN** it fails with the corresponding port error instead of ignoring the value or creating an undeclared default

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
the same rules for element or value types. Overlapping numeric ranges SHALL retain runtime checks when narrowing is unproven. No integer/floating representation,
string, or collection coercion SHALL occur; explicitly selected single-precision codecs SHALL round within their finite range.

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

### Requirement: Derive closed enum value codecs

Providers SHALL be able to derive string enum values and internally tagged object enum values for fields and collection
members. Variant and field renames SHALL be exact and independent of serialization attributes. Unknown variants,
invalid tags, malformed payload fields, unsupported variant shapes, and ambiguous names MUST fail conversion or
compilation. Conversion failures SHALL preserve typed causes and escaped pointers. Enum codecs SHALL remain outside
certified direct transfer.

#### Scenario: Round-trip enum payloads

- **WHEN** a value bag contains renamed string variants and tagged unit or named payload variants in a list
- **THEN** both directions retain wire names, declared payload fields, and omission semantics

#### Scenario: Reject invalid enum declarations

- **WHEN** an enum declares tuple payloads, empty or duplicate wire names, or a payload field colliding with its tag
- **THEN** derivation fails with a compile-time diagnostic

#### Scenario: Preserve enum failure provenance

- **WHEN** a tagged value has an unknown or missing tag, unknown payload field, or invalid numeric descendant
- **THEN** conversion fails with the relevant escaped field pointer and typed directional cause

### Requirement: Apply explicit field defaults only to absence

Input and unified value derives SHALL support explicit Default or zero-argument factory defaults. Only absent bindings
SHALL invoke the default. Supplied values MUST retain strict decoding, including null checks. Defaulted ports SHALL be
optional; unified contracts SHALL use that declaration in both roles and encode their actual field values. Direct
transfer MUST preserve default behavior rather than substituting an absent optional value.

#### Scenario: Decode an absent defaulted field

- **WHEN** a defaulted numeric or generic field has no binding
- **THEN** decoding calls its declared default and succeeds without making the port required

#### Scenario: Preserve supplied invalid data

- **WHEN** a defaulted unsigned field receives explicit null or an invalid representation
- **THEN** decoding fails instead of replacing the supplied value with the default

#### Scenario: Preserve an optional factory value in generated execution

- **WHEN** an unconnected optional field declares a factory returning a present value
- **THEN** generated and dynamic execution both observe that value instead of absence

### Requirement: Distinguish nullable values from omitted fields

Nullable value codecs SHALL accept explicit null or a valid inner value, including collection members. Optional
nullable fields SHALL distinguish absence, supplied null, and supplied values in both directions. The nullable
constructor SHALL participate in descriptor parsing, validation, compatibility, and the shared nesting bound. Encoding
and certified validation MUST reject an inner value encoding as null so direct and dynamic transfer remain equivalent.

#### Scenario: Round-trip three presence states

- **WHEN** an optional nullable field is omitted, null, or a supplied string
- **THEN** conversion preserves respectively absence, explicit null, or the supplied string

#### Scenario: Validate nullable collection members

- **WHEN** a list contains explicit null and valid unsigned values under a nullable element descriptor
- **THEN** both conversion directions retain those members and reject invalid non-null values

#### Scenario: Reject ambiguous null representations

- **WHEN** a nullable value variant wraps a raw null, nullable null, or shared null payload
- **THEN** dynamic encoding and certified validation both reject it with the same failing field path

### Requirement: Check unsigned and single-precision numeric codecs

Unsigned 64-bit, target-sized unsigned, and single-precision fields SHALL expose uint, usize, and float descriptors.
Unsigned decoding MUST reject negative integers, floats, and out-of-range values. Single-precision decoding SHALL accept
finite floating numbers within its finite range and round to that precision. Encoding and certification MUST reject
non-finite floats, preserve integer values, and retain negative zero. Recursive errors SHALL retain their pointers.

#### Scenario: Preserve numeric limits

- **WHEN** values contain unsigned limits, target-sized limits, finite single-precision limits, or negative zero
- **THEN** their codecs retain the integer value or single-precision bits through a round trip

#### Scenario: Reject incompatible representations and ranges

- **WHEN** an unsigned field receives a negative or floating number, or a single-precision field receives an integer or excessive magnitude
- **THEN** decoding fails with the declared numeric descriptor and failing path

#### Scenario: Reject non-finite output values

- **WHEN** a single-precision field or collection member contains infinity or NaN
- **THEN** encoding and certified validation fail rather than replacing the number with null

### Requirement: Defer owned decoding of shared typed payloads

A shared typed view SHALL retain its original immutable payload and expose it for inspection before owned decoding.
Construction SHALL validate the wire descriptor without invoking the owned decoder; encoding and cloning SHALL preserve
payload identity. Broad object and enum descriptors SHALL defer strict field or variant checks to explicit decoding.
Only runtime-certified inner codecs SHALL qualify a shared view for certified direct transfer.

#### Scenario: Check a budget before owned decoding

- **WHEN** a node receives a shared typed list and inspects its length before decoding
- **THEN** no owned element decoder has run, and an explicit decode invokes it afterward

#### Scenario: Forward a shared typed payload

- **WHEN** a shared view is cloned or emitted as an output
- **THEN** its immutable payload identity remains the same without calling the owned decoder

#### Scenario: Reject an invalid deferred object

- **WHEN** a shape-valid shared object violates its declared field or enum contract
- **THEN** explicit decoding fails with the strict nested codec's typed cause and pointer
