# Proposal

## Why

`builtin.constant` and `builtin.identity` expose `Any` ports even when a workflow already determines their values or types. This hides definite connection errors until execution and prevents types from flowing through an identity node. CEL outputs are already inferred, so these core nodes should participate in the same graph validation.

## What Changes

- Infer a sound output type from each constant's configured JSON value, including homogeneous nested lists and string-keyed maps.
- Propagate type and exact-value evidence across identity nodes in dependency order, without executing nodes during validation.
- Reject every connection whose known value violates the target port contract, including heterogeneous collections that require a broad inferred descriptor. Reject incompatible inferred types before executable installation.
- Keep runtime checks for genuinely unknown broad or `Any` outputs, and give in-memory flows and generated binaries the same resolved port metadata and diagnostics.
- Preserve existing node kinds, configuration, JSON results, workflow schema, and third-party registrations that do not opt into inference.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `core-nodes`: Expose constant-value evidence and identity pass-through evidence for graph type inference.
- `typed-port-contracts`: Distinguish known values and inferred types from unknown broad sources when checking connections.
- `workflow-binary-compilation`: Resolve graph-dependent port types before connection validation and use the same resolution in generated execution.

## Impact

- `mf-runtime` node metadata, `mfn-core` constant and identity nodes, and `mf-compiler` graph validation and generated runner setup.
- Existing workflows with provably invalid constant or identity connections will fail compilation instead of failing at execution.
- Workflow and plugin documentation and focused in-memory, generated-runner, and packaged-CLI tests.
