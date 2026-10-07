# Spec Delta

## ADDED Requirements

### Requirement: Derive typed input declarations from owned fields

Struct-defined inputs SHALL map bool, i64, f64, String, and ValueRef to Boolean, Int64, Float64, String, and Any ports. Vec and string-keyed BTreeMap fields SHALL recursively describe lists and maps. Ordinary fields SHALL be required; a top-level Option field SHALL retain its inner descriptor and be non-required. Declaration and decoding semantics SHALL agree, including through type aliases.

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
