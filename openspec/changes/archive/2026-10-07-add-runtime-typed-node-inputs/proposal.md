# Proposal

## Why

Node providers currently repeat their input contract in port metadata and manual map decoding. A runtime-owned typed input API lets a Rust struct define both declarations and execution inputs, keeping conversion mechanics out of provider business logic.

## What Changes

- Add runtime input contracts and field codecs that derive port types and required flags from supported owned Rust types.
- Add a `NodeInputs` derive macro, implemented in `mf-runtime-derive` and re-exported by `mf-runtime`.
- Add `TypedTaskNode` and a runtime-owned adapter to the existing dynamic task execution contract.
- Add a typed prepared-task constructor that generates input metadata and rejects competing input declarations.
- Preserve strict JSON typing, omission versus null, shared `ValueRef` payloads, typed error sources, and existing execution checks.
- Migrate `builtin.identity` as a reference provider and document the additive plugin API. Configuration-dependent providers retain their dynamic input interfaces.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `node-preparation`: Prepare typed task providers with runtime-generated input declarations and runtime-owned input conversion.
- `typed-port-contracts`: Define struct field mappings, strict typed decoding, optional field semantics, and shared-value preservation.

## Impact

Changes affect `mf-runtime`, a new workspace proc-macro package, workspace dependency declarations and lockfile, the identity provider, plugin documentation, and compiler integration fixtures. Existing task, event, and stream interfaces remain supported; workflow JSON, scheduling, startup interfaces, and output contracts retain their behavior. Typed stream/event interfaces, record-valued port types, output structs, and arbitrary Serde customization are outside this change.
