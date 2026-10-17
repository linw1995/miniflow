# Proposal

## Why

Code rejects heterogeneous JSON values, so workflows cannot use CEL list concatenation to collect structured model
usage records. The chat work adds `builtin.concat` to bridge this type gap; extending Code removes that duplication
and enables ordinary JSON transformations without chat-specific execution machinery.

## What Changes

- Extend Code inputs with existing `any`, `object`, `array`, and `uint` descriptors, including nested collections.
- Infer dynamic JSON output contracts and accept explicit `dyn(...)` while retaining static checks where possible.
- Convert heterogeneous JSON recursively without numeric precision loss or invented missing fields.
- Explicitly propagate input sensitivity into Code outputs using a conservative provider policy.
- Retain Code's existing resource limits and atomic output publication for dynamic values.
- **BREAKING**: Replace `builtin.concat` with `builtin.code` using `left + right`, then remove its registration,
  implementation, and exclusive tests. Migrated flows declare `mfn-code` and adopt Code's documented limits.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `code-node-execution`: support dynamic JSON ports, lossless conversions, and explicit sensitivity propagation.
- `core-nodes`: retire dedicated concatenation in favor of Code expressions.

## Impact

Primary implementation changes belong in `crates/builtin-nodes/code`, with existing compiler/runtime contracts reused.
Migration touches core registration, chat workflow examples and integration tests, package declarations, and docs.
No new dependencies, runtime types, schedulers, or output-schema configuration are planned.

The current checkout does not contain `builtin.concat` or its core requirement. Those exist on the local
`feat/chat-flow-runtime` branch associated with session `01a14599-d4c6-7f30-b114-5c5b7311cf07`.
Code work can proceed independently, but the removal delta and chat migration require that work to be integrated
before applying those steps or archiving this complete change. This proposal does not merge branches.
