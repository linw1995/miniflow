# Proposal

## Why

Input and output structs describe the same named data contract, but providers currently implement separate directional
APIs. Generated workflows also erase concrete task types and round-trip every typed connection through dynamic maps,
even when the compiler can prove direct Rust field transfer is safe.

## What Changes

- Introduce one `NodeValue` trait and derive for named owned port structs, with one declaration and bidirectional
  dynamic conversion. Input and output remain roles of a task's associated types.
- Keep existing directional contracts as compatibility adapters during migration; migrate built-in typed providers and
  documentation to the unified contract.
- **BREAKING**: Exhaustive `NodeMetadata` literals must include the new optional `typed_generation` field.
- Add optional provider-supplied typed code-generation metadata without requiring the CLI to know plugin implementations
  or making private implementation types public.
- Generate direct typed field transfers for proven compatible, single-consumer task connections inside a top-level
  oneshot execution domain.
- Retain dynamic conversion at startup, unsupported connections, context and observation boundaries, domain boundaries,
  and workflow outputs. Preserve strict validation, errors, presence, skips, and scheduling.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: Define unified port structs and the proof obligations for direct typed transfer without
  changing JSON semantics.
- `node-preparation`: Support unified typed contracts and optional generated typed construction while retaining dynamic
  providers and preparation boundaries.
- `workflow-binary-compilation`: Select and generate typed fast paths from linked provider metadata, with deterministic
  dynamic fallback.
- `workflow-runtime-execution`: Execute generated typed segments through the existing domain scheduler and node
  lifecycle, preserving validation and publication semantics.

## Impact

Affected areas are `mf-runtime`, `mf-runtime-derive`, `mf-compiler` plan/project generation, typed built-in providers,
external plugin fixtures, and node-development documentation. No workflow schema change, implicit coercion, new
scheduler, or dependency update is planned. Manifest inspection remains declarative; code-generation metadata is
build-time data rather than a portable Rust type identifier.

The first version excludes typed fan-out, cross-domain transfer, stream/event execution, nested Loop/Iteration fast
paths, automatic conversion between distinct Rust types, and arbitrary nested struct/enum codecs. Existing supported
field codecs remain available; opaque custom codecs use dynamic fallback until they supply a verifiable typed validation
contract.
