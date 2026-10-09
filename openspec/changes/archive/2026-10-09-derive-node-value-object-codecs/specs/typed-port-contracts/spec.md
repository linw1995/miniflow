## ADDED Requirements

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
