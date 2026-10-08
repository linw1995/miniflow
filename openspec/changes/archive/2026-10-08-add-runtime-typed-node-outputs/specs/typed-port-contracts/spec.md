# Spec Delta

## ADDED Requirements

### Requirement: Derive typed output declarations from owned fields

Struct-defined outputs SHALL map bool, i64, f64, String, and shared JSON values to Boolean, Int64, Float64, String, and Any ports. Lists and string-keyed maps SHALL recursively describe their values. Ordinary fields SHALL be required; top-level optional fields SHALL retain the inner descriptor and be non-required. Type aliases SHALL preserve these semantics.

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
