## Why

Larger workflows are difficult to scan when ordinary nodes form recognizable groups but the graph has no
display-level structure. Operators need short labels for folded groups and longer annotations for inspection without
changing execution semantics.

## What Changes

- Add definition version `2026-10-23` with optional `clusters` metadata in every graph scope.
- Define each cluster with a short `label`, a longer `annotation`, and scope-local member nodes.
- Carry cluster metadata through compiled descriptions and manifests without adding graph nodes or dependencies.
- Render cluster annotations in the terminal UI and let operators collapse or expand them.
- Keep cluster state session-local; it never changes workflow results, identity inputs, or event schemas.

## Capabilities

### Modified Capabilities

- `workflow-binary-compilation`: versioned, display-only cluster declarations in definitions and descriptions.
- `workflow-observability`: descriptions preserve cluster labels, annotations, and scope-local membership.
- `workflow-terminal-ui`: annotated cluster boundaries, aggregate collapsed nodes, and fold controls.

## Impact

Workflow definitions, compiled manifests, telemetry descriptions, TUI graph projection and interaction, examples,
documentation, and regression coverage. Runtime scheduling, node registries, port contracts, lifecycle events, and
snapshot formats are unchanged.
