## Why

`NodeValue` derives a named-port contract but does not implement the single-value codecs required by `Vec<T>` and
string-keyed maps. Providers cannot compose their derived structs into collection elements or nested fields.

## What Changes

- Derive object-valued `InputValue`, `InputField`, and `OutputValue` implementations alongside the existing contract.
- Reuse named-port conversion to retain strict field rules, shared payloads, and typed error chains.
- **BREAKING**: extend `TypeMismatch` from a struct to a source-bearing enum. Existing field reads remain available;
  direct construction and destructuring require migration.
- Extend existing runtime regressions and document object descriptors and typed-generation boundaries.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: compose derived unified values as JSON objects and preserve nested conversion failures.

## Impact

Changes affect `mf-runtime`, `mf-runtime-derive`, runtime tests, and node development documentation. The JSON descriptor
vocabulary and certified typed-generation representations remain unchanged. No dependencies are added.
