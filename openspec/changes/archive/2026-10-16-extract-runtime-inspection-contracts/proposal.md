## Why

Hosts need node kinds and configuration from compiled workflows without invoking providers. The compiled definition
already contains this information; runtime inspection should reuse it instead of introducing a second configuration
publication contract.

## What Changes

- Embed the existing compiled definition in manifest version `2026-10-16`.
- Add `WorkflowManifest::node_definition` for borrowed root-node lookup by exact ID.
- Reuse the manifest definition for runner plan inspection and remove the separate runtime plan JSON embedding.
- Expose the existing template evaluator and unique-key JSON decoder for host callers.
- Preserve legacy manifest reading and update the generated-project cache layout.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-binary-compilation`: inspect and reuse the original compiled definition through the manifest.

## Impact

Runtime manifest contracts, compiler generation, binary inspection tests, and documentation. The manifest contains
full compiled configuration rather than an author-selected public subset. Runtime arguments are not persisted into it.
Rust manifest literals acquire a definition field; existing serialized manifests remain supported.
