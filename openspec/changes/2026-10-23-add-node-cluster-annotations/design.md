## Context

The terminal UI already projects one described scope at a time and supports entering Loop and Iteration bodies. Node
groups need an orthogonal display concept: they annotate an existing subgraph, can be folded, and never introduce a
container node or an execution boundary.

## Decisions

- **Definition metadata, not graph structure.** A `clusters` array accompanies `nodes` in the root, Loop bodies, and
  Iteration bodies. Clusters never create scheduling dependencies, input bindings, ports, or synthetic execution nodes.
- **Short and long text are separate.** `label` is the required single-line graph title. `annotation` is the required
  longer explanation wrapped inside expanded and collapsed clusters and repeated in Details. Graph widths reserve the
  complete label, and annotation rows occupy routing obstacles so connections cannot overwrite the text.
- **No definition-side identifier.** Declaration order identifies a cluster within its immutable scope. The TUI assigns
  an internal `ClusterKey` from the scope path and declaration ordinal for folding, selection, and edge remapping.
- **Flat clusters.** A node belongs to at most one cluster, and clusters cannot nest. Nested folding can be added later
  without changing the current validation or stored metadata.
- **Version-gated metadata.** Definition and description version `2026-10-23` carry clusters. Older readers reject the
  description version before execution, and older definitions cannot declare clusters. Event schema versions do not
  change.
- **Condensed layout.** Collapsing a cluster builds one display vertex containing its member source indices. Data,
  control, and success edges whose endpoints map to the same display vertex are hidden; external edges remap to the
  cluster vertex. Aggregate status prefers failure, then running, uncertainty, pending, success, and skip.
- **Explicit bounds.** Cluster count, label bytes, annotation bytes, member existence, uniqueness, and synthetic-source
  exclusion are validated in definitions and descriptions.
- **Session-local presentation state.** Fold state is retained per scope and connection view for the current TUI
  session. It is not serialized into manifests, snapshots, or telemetry.

## Tradeoffs

Expanded clusters occupy separate vertical bands while retaining dependency columns, so their frames cannot overlap
or enclose unrelated nodes. Bands follow group dependencies when their projection is acyclic. Dense connections can
still cross a frame between groups. Labels overwrite only their own text cells, preserving the rest of the top border.
The leftmost expanded member reserves annotation rows above its visible box; folded
vertices reserve those rows below their summary. This keeps the text inside existing routing obstacles without adding
display vertices or execution edges. Flat clusters keep folding deterministic; nesting
would require hierarchical layout and selection mapping. Recompiling a `2026-10-23` workflow selects a new description
version even when it declares no clusters, which is acceptable because the schema version is explicit.

Transitive reduction uses the projected graph's topological order rather than aggregate display indices. Folding can
also introduce a display cycle through nodes outside a cluster; in that case all projected dependencies are retained.

## Current interfaces

`WorkflowDefinitionVersion::V2026_10_23`, `ClusterDefinition`, `ClusterDescription`, `GraphCluster`,
`GraphLayout::with_view_and_collapsed`, and `GraphLayout::from_body_with_view_and_collapsed` expose the feature.
`ObservationView` owns collapse state and handles the `c` key.
