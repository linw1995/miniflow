# Proposal

## Why

The current `ValueType` distinguishes only broad JSON categories, so the compiler cannot express an integer, a floating value, or homogeneous nested collections. CEL Code nodes need these types for useful static checking, and the same contracts should be available to every node rather than duplicated inside one package.

## What Changes

- Extend shared port types with signed 64-bit integers, 64-bit floating values, typed lists, and string-keyed typed maps while preserving existing `Any`, `Number`, `Array`, and `Object` descriptors.
- Classify connections as statically safe, dynamically checked, or incompatible. Broad or `Any` outputs may feed refined inputs only with a runtime type guard; concrete mismatches fail compilation.
- Validate produced outputs and bound inputs against their declared types before node execution or context publication, with recursive diagnostics and unchanged skip/missing-output precedence.
- Let both in-memory and generated execution use the same checks, and update the CEL Code plan to expose its declared types through the shared port contract.
- **BREAKING**: Plugins that currently return values inconsistent with declared output types will fail execution. Recursive `ValueType` values will no longer be `Copy`; Rust callers relying on copies must clone or borrow them.

## Capabilities

### New Capabilities

- `typed-port-contracts`: Describe recursive JSON port types, gradual connection compatibility, and runtime boundary checks.

### Modified Capabilities

- `workflow-binary-compilation`: Replace the exact-type-only connection rule with static-safe and runtime-checked compatibility while retaining validation and generated-runner guarantees.

## Impact

- `mf-runtime` port metadata and shared execution checks; `mf-compiler` edge validation and diagnostics.
- Built-in and third-party node packages using `ValueType`, especially CEL Code nodes with refined instance ports.
- Workflow/plugin documentation, compiled-binary tests, and migration guidance for the Rust API and stricter runtime validation.
