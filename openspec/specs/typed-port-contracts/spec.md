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

### Requirement: Expose optional output derivation metadata

Nodes MAY declare that an output is an exact configured JSON value, an unchanged copy of a named input, a collection of values from a named input, or a type proven by a prepared contract.
Validation SHALL use such declarations only for the node's existing ports and MUST reject malformed derivation
metadata or an exact value that contradicts its own output declaration, with node and port context. Nodes that
provide no derivation metadata SHALL retain their reflected port types and existing
runtime boundary checks. Execution SHALL continue to validate actual produced and bound values against the
resolved ports.

A collection derivation SHALL wrap the input type as `List(T)`, retain dynamic boundary checks for broad elements, and discard exact-value evidence for the collected output. It MUST reference an existing input and output, fit the output declaration, and respect the shared type-depth limit. Generated and in-memory preparation SHALL use the same derivation rules without node-kind-specific inference.

#### Scenario: Preserve an ordinary plugin

- **WHEN** a plugin declares an `Any` output and supplies no output derivation
- **THEN** a connection to an `Int64` input remains runtime-checked

#### Scenario: Reject an invalid derivation

- **WHEN** a plugin derives an output from a nonexistent input port
- **THEN** build validation fails before installation and identifies the plugin node and invalid port reference

#### Scenario: Reject a contradictory literal declaration

- **WHEN** a plugin declares a `String` output but derives it from an exact integer literal
- **THEN** build validation rejects the node metadata before installation

#### Scenario: Guard a false derivation

- **WHEN** a plugin declares an output derivation but produces a value outside its resolved output type
- **THEN** execution fails before publishing that output

#### Scenario: Derive a collection element type

- **WHEN** a node declares collection-of-input evidence for an input bound to `Int64`
- **THEN** its output resolves to `List(Int64)` without a known exact array value

#### Scenario: Keep unknown elements checked

- **WHEN** an input bound to `Any` is collected and its output feeds `List(Int64)`
- **THEN** the connection retains runtime element checks and an invalid element reports its array index

#### Scenario: Reject excessive derived nesting

- **WHEN** wrapping the bound input type in a list exceeds the descriptor depth limit
- **THEN** validation fails with the deriving node and output port

A proven-type derivation SHALL narrow the descriptor of an existing reflected output without changing its name or
requiredness. It SHALL carry type evidence without an exact value and MUST respect the shared descriptor-depth limit.
Compilation and runtime publication SHALL enforce it through the existing validation path.

#### Scenario: Resolve a type proven by a prepared body

- **WHEN** an output field reflects `List(Any)` and its prepared body proves `List(Int64)`
- **THEN** compiler inference resolves `List(Int64)` without treating the output as an exact known array

#### Scenario: Reject widening type evidence

- **WHEN** an `Int64` output supplies proven-type evidence for `Any` or `String`
- **THEN** validation rejects the evidence with node and output context

#### Scenario: Guard a false proven type

- **WHEN** an output proves `Int64` but business execution returns a string
- **THEN** runtime publication rejects the result before exposing any output

### Requirement: Preserve typed compiler validation sources

Compiler port and derivation validation failures SHALL retain their original typed error sources and source chains, together with node and port context. Diagnostic formatting SHALL NOT replace the underlying errors with strings.

#### Scenario: Preserve a contradictory literal's nested mismatch

- **WHEN** output derivation validation rejects a literal element that contradicts the declared collection type
- **THEN** the compiler error identifies the node and retains both `OutputDerivationError` and its `TypeMismatch` source with the failing JSON Pointer

#### Scenario: Preserve a forwarded known-value mismatch

- **WHEN** an exact forwarded value contradicts its output declaration
- **THEN** the compiler error identifies the node and output and retains the `TypeMismatch` source

#### Scenario: Preserve excessive port depth

- **WHEN** a declared port type exceeds the shared nesting limit
- **THEN** the compiler error identifies the node, port, and direction and retains the `TypeDepthError` source

### Requirement: Preserve typed runtime port validation sources

Runtime input and output port validation failures returned as `WorkflowRunError` SHALL retain the original `TypeMismatch` as their error source, together with the node ID and input or output port name. Callers SHALL be able to inspect the failing JSON Pointer, expected type, and actual type through that source. Diagnostic formatting SHALL NOT replace the underlying error with a string.

#### Scenario: Preserve an invalid producer output

- **WHEN** a plugin declares an `Int64` output but produces a JSON string
- **THEN** execution fails with the producer node and output name, retains a `TypeMismatch` source, and publishes none of that node's outputs

#### Scenario: Preserve a nested dynamic input mismatch

- **WHEN** a `List(Map(Int64))` input receives `[{"count": 1}, {"count": "two"}]`
- **THEN** execution fails before invoking the consumer, identifies its node and input name, and retains a `TypeMismatch` source with path `/1/count`, expected type `Int64`, and actual type `string`

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

### Requirement: Define supported struct shapes and port identities

The input derive SHALL support named-field structs, including empty structs and generics constrained by supported field codecs. Field identifiers SHALL define port names unless explicitly renamed; raw identifiers SHALL use their unprefixed name. Unsupported shapes or field types and empty or duplicate port names MUST produce compile-time diagnostics. Providers SHALL be able to select an explicit runtime dependency path.

#### Scenario: Rename a field

- **WHEN** a provider gives a supported field an explicit port name containing dots, slashes, or tildes
- **THEN** declarations and decoding use that complete name without splitting it and error pointers escape it correctly

#### Scenario: Normalize a raw identifier

- **WHEN** a field identifier is a Rust raw identifier for a keyword
- **THEN** the port name omits the raw identifier prefix

#### Scenario: Prepare an empty input struct

- **WHEN** a task uses an empty named-field input struct and receives an empty input map
- **THEN** its input declaration is empty and decoding succeeds

#### Scenario: Reject unsupported declarations

- **WHEN** a provider derives inputs for an enum, tuple or unit struct, borrowed field, unsupported field type, or fields with empty or duplicate port names
- **THEN** compilation fails with a diagnostic identifying the unsupported declaration

#### Scenario: Compile a renamed runtime dependency

- **WHEN** an external provider renames its runtime dependency and supplies the explicit runtime path to the derive
- **THEN** the generated input implementation compiles against that dependency without requiring a second runtime identity

### Requirement: Decode typed inputs without changing JSON semantics

Typed input decoding SHALL reject missing required fields, unknown fields, and values outside the declared descriptor without coercion or defaults. Missing optional fields SHALL decode as None; supplied values SHALL decode as Some of the decoded inner value. Explicit null SHALL remain supplied data subject to the inner descriptor. Recursive validation SHALL honor existing type depth limits.

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

- **WHEN** a floating-point field receives an integer JSON number or an integer field receives a floating JSON number
- **THEN** decoding rejects the value under the existing Float64 or Int64 contract without coercion

#### Scenario: Bound recursive declarations

- **WHEN** a derived collection descriptor exceeds the shared type depth limit
- **THEN** preparation validation rejects it with the existing typed depth error

### Requirement: Preserve shared payloads during typed decoding

Typed decoding SHALL consume the input map without serializing it to text or materializing an intermediate owned JSON tree. ValueRef fields SHALL retain their shared payloads. Shared JSON descendants returned through typed collections SHALL retain their payload identity; constructing owned strings and typed collection containers SHALL remain allowed.

#### Scenario: Forward a shared input

- **WHEN** the typed identity provider forwards its ValueRef input
- **THEN** the output retains the same shared payload identity and JSON value

#### Scenario: Retain nested shared values

- **WHEN** typed decoding creates a list or string-keyed map of ValueRef values
- **THEN** its elements or values share their payloads with the corresponding input descendants

### Requirement: Preserve typed input decoding error sources

Runtime-owned input decoding failures SHALL expose the affected port and preserve typed causes and source chains. Invalid values SHALL retain their TypeMismatch source, expected and actual types, and failing JSON Pointer. Workflow execution errors SHALL retain node attribution around decode failures. Provider business failures SHALL continue to use their existing plugin error contracts.

#### Scenario: Inspect a nested decode failure

- **WHEN** decoding a list of integer maps encounters a string at /1/count
- **THEN** the decode error identifies the input port and retains a TypeMismatch source with path /1/count and the expected integer type

#### Scenario: Attribute an adapter failure

- **WHEN** a prepared typed task fails during runtime input conversion
- **THEN** business logic is not invoked and its workflow error retains the node identity and typed decode error source

#### Scenario: Preserve a provider business failure

- **WHEN** typed business logic returns its existing source-bearing plugin error
- **THEN** the runtime preserves that error chain without wrapping it as an input decoding failure

### Requirement: Derive typed output declarations from owned fields

Struct-defined outputs SHALL map bool, i64, f64, String, and shared JSON values to Boolean, Int64, Float64, String, and Any ports. Lists and string-keyed maps SHALL recursively describe their values. Ordinary fields without explicit unified-contract defaults SHALL be required; top-level optional fields SHALL retain the inner descriptor and be non-required. Type aliases SHALL preserve these semantics.

#### Scenario: Declare recursive output values

- **WHEN** an output struct contains a list of string-keyed integer maps
- **THEN** its port is a required List(Map(Int64)) and encoded values satisfy that descriptor

#### Scenario: Declare optional output values

- **WHEN** an output struct contains an optional string field or a supported type alias
- **THEN** its port is non-required String with the same encoding semantics as the underlying type

### Requirement: Define supported output struct shapes and port identities

Output derivation SHALL support owned named-field structs, empty structs, generics, exact field renames, and an explicit runtime dependency path. Raw identifiers SHALL use their unprefixed names. Unsupported shapes or field types, borrowed fields, malformed attributes, and empty or duplicate port names MUST produce compile-time diagnostics. Output attributes SHALL remain independent of serialization attributes.

#### Scenario: Preserve renamed and raw identifiers

- **WHEN** an output field has a renamed port containing punctuation or uses a raw Rust identifier
- **THEN** declarations and encoding retain the exact renamed name or the unprefixed identifier

#### Scenario: Encode empty outputs

- **WHEN** an empty named-field output struct is encoded
- **THEN** it declares no ports and produces an empty output map

#### Scenario: Compile an aliased runtime dependency

- **WHEN** an external provider derives generic outputs against an explicitly renamed runtime dependency
- **THEN** its output implementation compiles against the selected runtime identity

#### Scenario: Reject unsupported output fields

- **WHEN** a provider derives outputs containing unsupported types, optional collection elements, nested options, borrowed fields, or ambiguous port names
- **THEN** compilation fails with a diagnostic identifying the unsupported declaration

### Requirement: Encode typed outputs without changing JSON semantics

Typed output encoding SHALL preserve scalar representations and recursively encode supported collections without coercion or an intermediate owned JSON tree. Absent optional fields SHALL omit their binding; present shared null values SHALL remain data. Omission SHALL NOT itself mark a port skipped. Non-finite floating values and descriptors exceeding the shared depth limit MUST fail encoding.

#### Scenario: Distinguish omission from null

- **WHEN** an optional shared output is absent or contains explicit JSON null
- **THEN** the absent field is omitted and the present null is emitted as data

#### Scenario: Preserve numeric representations

- **WHEN** an output contains signed integer limits or a finite floating value including negative zero
- **THEN** encoding retains the integer or floating representation and value

#### Scenario: Reject a non-finite descendant

- **WHEN** a floating output or a nested list/map value is non-finite
- **THEN** encoding fails instead of replacing that value with JSON null

#### Scenario: Bound optional declarations before encoding

- **WHEN** an optional output field has a descriptor exceeding the shared nesting limit even while absent
- **THEN** encoding fails with a typed descriptor-depth source

### Requirement: Preserve shared output payloads and encoding sources

Output encoding SHALL preserve shared JSON payload identity for raw fields and collection descendants. Encoding failures SHALL retain the port, typed mismatch or depth source, and escaped nested pointer. Workflow failures SHALL retain the producer identity. Provider business errors SHALL retain their existing source chains.

#### Scenario: Retain shared output descendants

- **WHEN** a typed task emits shared JSON directly or through lists and maps
- **THEN** the published descendants retain the corresponding shared payload identities

#### Scenario: Inspect an encoding failure

- **WHEN** an invalid nested floating value occurs under renamed ports and escaped map keys
- **THEN** its error preserves the typed cause and full escaped path through the node execution error

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

### Requirement: Fixed builtin port bags use the unified contract

Constant, Batch, and Readline SHALL use `NodeValue` bags and typed execution contracts. Their factories MUST NOT
handwrite `NodePortContract` or override ports. Constant SHALL preserve literal evidence and shared values. Batch SHALL
reflect `List(Any)` before collection inference and retain flush behavior. Readline SHALL preserve its optional non-null
path and stdin ownership. IfElse, Loop, and Code SHALL reflect validated dynamic contracts.

#### Scenario: Preserve constant evidence and shared values

- **WHEN** a configured Constant produces a value consumed by Identity
- **THEN** literal type evidence is preserved and Identity retains the original shared value handle

#### Scenario: Preserve event and stream boundaries

- **WHEN** Batch accepts items or Readline receives an optional path
- **THEN** runtime-owned typed conversion preserves batching semantics and text source ownership without enabling task-only generated segments

#### Scenario: Keep fixed declarations independent of output evidence

- **WHEN** Constant, Batch, or Readline is prepared without invocation values
- **THEN** its port names, field descriptors, and requiredness come from the declared value bags while evidence is preserved separately

#### Scenario: Change a fixed execution field

- **WHEN** a fixed builtin changes a field in its execution-associated input or output value type
- **THEN** preparation reflects the corresponding contract without updating a separate port declaration

### Requirement: Builtin Identity advertises certified generation

Identity SHALL expose a provider-owned constructor for its certified input and output fields. Eligible standard oneshot Identity chains SHALL use direct field moves through the shared lifecycle. Refined, observed, scoped, and stream paths SHALL retain the existing conservative dynamic fallback.

#### Scenario: Compile and execute an Identity chain

- **WHEN** a standard oneshot workflow connects sole-consumer Identity nodes and selects only the terminal output
- **THEN** generated execution uses an eligible typed segment and returns the same selected value as dynamic execution

#### Scenario: Retain configured refinement fallback

- **WHEN** a Constant establishes a concrete refinement upstream of an Identity chain
- **THEN** unsupported refinement proof retains dynamic execution and the configured workflow result remains unchanged

### Requirement: Compose derived unified values as JSON objects

The unified value derive SHALL supply object input/output codecs and a required input field implementation. Derived
values SHALL compose as direct or optional fields, list elements, and string-keyed map values. Their broad `Object`
descriptor SHALL delegate strict field conversion to the named-port contract while preserving shared descendants.
These codecs MUST NOT automatically certify custom structs for generated direct transfer.

#### Scenario: Round-trip objects in collections

- **WHEN** a unified value contains lists and string-keyed maps of a generic derived value with runtime aliases and
  exact field renames
- **THEN** its nested objects encode and decode with the declared field identities and retain shared descendant identity

#### Scenario: Preserve optional object fields

- **WHEN** a derived object field is optional and absent
- **THEN** its binding is omitted and decoding returns absence rather than supplying an empty object or JSON null

#### Scenario: Encode an empty value as an object

- **WHEN** a list contains an empty named-field unified value
- **THEN** that element encodes as an empty JSON object and decodes back into the empty value

#### Scenario: Retain dynamic validation for custom objects

- **WHEN** a field uses the object codec of a derived custom struct
- **THEN** codec availability alone does not establish certified typed-transfer equivalence

### Requirement: Preserve nested object conversion failures

Derived object conversion SHALL preserve strict named-port validation, descriptor depth limits, and typed directional
causes. Failures SHALL expose a JSON Pointer spanning enclosing ports, indices, escaped map keys, and renamed fields.
Non-object inputs SHALL fail without panicking; missing and unknown fields SHALL retain their typed port errors.
Nested scalar mismatches SHALL retain expected and actual types.

#### Scenario: Reject a non-object element

- **WHEN** a list of derived objects receives a null or array element
- **THEN** decoding fails with an object mismatch at that element's pointer instead of panicking

#### Scenario: Retain structural object causes

- **WHEN** an object element omits a required renamed field or supplies an unknown field
- **THEN** decoding retains the missing-field or unknown-field source and its complete escaped pointer

#### Scenario: Retain nested scalar mismatch details

- **WHEN** a renamed integer field in a list element receives a string
- **THEN** decoding retains the expected integer type, actual string type, and the pointer through the object field

#### Scenario: Retain non-finite output provenance

- **WHEN** a non-finite floating field occurs inside an object under a list and escaped map key
- **THEN** encoding retains the original typed output error and mismatch with the complete nested pointer

#### Scenario: Retain descriptor-depth provenance

- **WHEN** a nested object's optional field has an over-depth descriptor while absent
- **THEN** both conversion directions fail with the typed depth source and the nested field pointer

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
