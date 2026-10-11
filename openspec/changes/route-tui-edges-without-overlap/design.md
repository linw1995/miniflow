## Context

See [proposal.md](proposal.md) for motivation and the [terminal UI delta](specs/workflow-terminal-ui/spec.md) for behavior contracts.

`GraphLayout::new` currently uses `rust-sugiyama` for layer and sibling order, then projects nodes into fixed 24-by-6 boxes with a ten-column gap. Edge attachment rows wrap modulo four. `GraphView::draw_edge` constructs midpoint paths while rendering, writes control labels over those paths, and lets later edges overwrite earlier cells. Node drawing follows edge drawing and can obscure long connections.

Layout is already created once for the root and each Loop body. Observation admission supports up to 10,000 nodes; graph-sized dense grids and unrestricted visibility graphs are unsuitable defaults.

## Goals / Non-Goals

**Goals:** retain the layered flow direction; plan integer-cell routes separately from rendering; eliminate collinear overlap in completed routes; minimize crossings without excessive detours; make remaining crossings readable and stable; bound work on large or congested inputs.

**Non-Goals:** global crossing optimality, planarization, implicit edge bundles or hyperedges, interactive graph editing, an external C++ or Java routing dependency, telemetry changes, and a full PathFinder implementation in the first delivery.

## Decisions

### 1. Preserve layering and make terminal projection routing-aware

For graphs with at least 512 nodes, execute the recursive Sugiyama projection on a joined worker with a fixed 64 MiB
stack. This avoids overflowing a caller thread with a small stack at the existing 10,000-node admission limit; worker
startup errors retain their typed I/O source. Retain the existing Sugiyama topology input, including deduplication of
parallel node pairs only for node placement. Keep observation indices tied to execution order. Derive stable edge keys
from source/destination IDs, source output, target input, and kind; use them for routing order and tie-breaking without
reordering observation records.

Allocate a separate attachment row per incident edge, including edges from the same output. Sort attachments by the
opposite endpoint's layer and sibling position, with stable keys for ties. Increase node height to cover the larger
input/output attachment count while keeping title and detail space. Use six-row attachment spacing with output/input
offsets of one/four and quantize per-component vertical placement to that spacing, so unrelated horizontal access runs
cannot occupy the same row. Use actual node heights when stacking and centering layers; remove fixed-height assumptions
from projection and node rendering.

Represent long-edge passage through intermediate layers using routing-only waypoints and reserved slots. These are not workflow nodes and never acquire observation state. Allocate their space before routing so long edges do not cross node boxes. Recompute horizontal positions after channel capacities are known, then freeze geometry for the scope.

Alternative: enlarging boxes after placement preserves existing coordinates but creates node overlap. Replacing the entire layout with ELK introduces an unnecessary execution and dependency boundary for this change.

### 2. Use channel routing and ordering as the primary algorithm

Model each layer gap as a channel. Collect vertical segment intervals and allocate integer-column tracks with a sorted
sweep. Overlapping closed intervals cannot share a track; include corner and marker clearance in interval conflicts.
Track order considers source/target attachment ordering and route direction to avoid unnecessary crossings. Tracks are
reused between disjoint interval regions; within each connected conflict region, ordered tracks avoid the bend-adjacent
crossings caused by unconstrained interval coloring. Unconstrained interval reuse alone is insufficient because
horizontal access segments can still cross neighboring tracks.

Preserve each pair's relative order through shared route regions before separating them. Treat nudging as integer track assignment with obstacle boundaries and minimum separation constraints, not independent rounding of continuous coordinates. Solve horizontal and vertical separation with a final geometric validation pass, since changing one direction can affect the other. Expand channels or reserved passage rows when constraints cannot fit.

Reserve control-label spans and endpoint arrows as occupied geometry. Sanitize label text and compute display widths using the terminal rendering rules. Prefer a label on an exclusive source stub; define deterministic truncation or another reserved position when the full label cannot fit. Include any label placement change in the final conflict check.

Alternative: one midpoint per edge cannot distinguish occupied tracks. Pure per-edge shortest paths encourage shared corridors and do not solve multi-edge separation.

### 3. Use bounded directional A* for exceptional routes

For blocked long edges or channels, search a component-local sparse orthogonal graph derived from node boundaries, ports, channel tracks, and reserved passages. A state includes arrival direction so bends have a real cost. Use a nonnegative cost combining length, bends, perpendicular crossings, and congestion, with Manhattan distance as an admissible lower bound for the length term. Do not claim global multi-edge optimality.

Nodes, exclusive endpoint geometry, labels, and invalid marker neighborhoods are hard obstacles. A crossing can be accepted only through the straight interior of another edge with sufficient clearance and no third participant. Temporary same-direction sharing may be considered during planning, but must be removed by track separation or rerouting before a route is accepted.

Perform a fixed, bounded number of deterministic local rip-up-and-reroute rounds. Include nearby blocking routes, not just routes currently reporting conflicts. Retain the best validated arrangement under a documented score: unresolved edges, overlaps, invalid contacts, crossings, bends, then length. Completed routes have zero overlaps and invalid contacts. Keep route validity separate from optimization scoring.

Alternative: Lee-style full-grid search consumes work proportional to canvas area. A global orthogonal visibility graph can grow quadratically in obstacle count. Full negotiated-congestion routing is deferred until measurements demonstrate that channel allocation and local retries are insufficient.

### 4. Store geometry and intersection ownership before rendering

Extend the owning graph representation with edge polylines, maximal axis-aligned segments, label rectangles, crossing records, unresolved-edge reasons, and canvas bounds. Keep current module paths and accessors unless implementation needs a specific API extension. Do not introduce facade modules or narrow existing visibility just to hide routing types.

Classify conflicts geometrically: node/label intrusion, collinear overlap, endpoint contact, bend collision, valid
perpendicular crossing, and marker-footprint collision. Shared outputs are not an implicit junction exception. Use
augmented AVL interval trees within row/column indexes, including vertical ranges for node/label/arrow rectangles, and
spatial sweeps for candidate conflicts, with explicit limits on pathological intersection counts. Avoid all-pairs edge
comparison and canvas-sized storage.

Build crossing layers from a stable total order: data above control, then canonical edge key within the same kind. This also avoids cyclic edge-wide layer assignments. Execution state changes style only. Each crossing stores both edge identities, their orientations, and the complete marker footprint.

Render each visible cell from planned geometry and ownership. Compose markers explicitly, selecting each stroke's own edge style, rather than inspecting or overwriting existing buffer symbols. Clip the result to the viewport; do not reroute on resize or pan. Expand canvas bounds to include passages, labels, markers, and outer detours.

### 5. Select the terminal crossing pattern through a focused prototype

The marker contract is fixed, but exact glyphs remain a presentation decision. Prototype both a multi-cell curved/rectangular bridge and a continuous upper stroke with explicit capped lower stubs. Compare horizontal-over-vertical and vertical-over-horizontal, all data/control combinations, and monochrome/status-colored rendering. A plain overwritten center cell, junction glyph, or unexplained gap does not satisfy the contract.

Check glyph cell width and visual alignment in representative monospaced terminals before selecting a pattern. Record the chosen patterns, footprint, clearance, and any fallback in this document. Perform this task before final track spacing and marker-aware conflict rules are implemented.

The marker footprint participates in route planning; rendering cannot enlarge it after routes are accepted. Dense crossings must move or become explicitly unresolved rather than degrading into ambiguous symbols.

This prototype step is part of implementation and does not require another approval round. No claim is made that a single Unicode character can encode both edge kinds and both crossing orientations.

#### Selected marker

Use the capped underpass in a 3-by-3 footprint. The upper stroke remains continuous. A lower vertical control edge ends at `┴`/`┬`; a lower vertical data edge uses `╧`/`╤`. A lower horizontal control edge uses `┤`/`├`; a lower horizontal data edge uses `╢`/`╟`. Every marker glyph occupies one terminal cell. Require three cells of straight-segment clearance from bends and endpoints, and disjoint marker footprints.

The preview matrix covers both orientations and all four edge-kind combinations. The compact caps preserve the lower edge's line kind and show deliberate interruption in monochrome; the bridge requires a larger footprint and displaces the upper path. The initial selection was visually inspected with Menlo. Local evidence lives in `target/edge-routing/crossings.txt`, `crossings.ansi`, and `crossings.png`; preview commands use `graph_preview -- --crossings [--color]`.

### 6. Make resource exhaustion explicit and separate from workflow failure

Introduce named limits for sparse graph vertices/arcs, cumulative search expansions per scope, retry rounds, retained
conflict records, and checked canvas extents. Enforce both per-search and scope-wide limits, including preprocessing and
marker enumeration. Defaults use 250,000 geometry items, 4,096 search vertices, 16,384 implicit arcs, 8,192 expansions
per search, 65,536 expansions per scope, 2,000,000 conflict checks, 16,384 retained crossings, 128 points per route, two
retry rounds, and a 2,000,000-cell route extent. Use deterministic operation counts rather than wall-clock cutoffs for
reproducible geometry.

Try a validated outer detour within the remaining canvas/work budget before marking an edge unresolved. Retain every
unresolved edge and its typed reason. Display a routing warning separately from telemetry completeness, and expose
endpoint/port/kind details when inspecting an incident node. Place the warning on its own row in the graph pane so it
remains visible in narrow terminals. Detail scrolling uses a bounded cell-width window and a machine-sized offset rather
than a 16-bit paragraph offset, retaining access to large incident-edge lists. Do not render a false complete path or
change workflow outcome.

Preserve a usable node layout when optional routing work is exhausted. Follow existing Snafu conventions for fallible layout operations and preserve typed sources.

## Risks / Trade-offs

- Larger boxes and channels increase panning -> reuse nonconflicting tracks and prefer short validated routes, while treating traceability as more important than compactness.
- Separation can introduce new intersections -> preserve segment order, validate all affected geometry, and use bounded retries or channel expansion.
- Marker patterns can obscure line-kind differences or look like disconnected edges -> compare both orientations, mixed kinds, and monochrome prototypes before freezing the footprint.
- Large components can exhaust sparse-graph or intersection budgets -> scope-wide accounting, interval indexes, and explicit unresolved-edge inspection.
- Local optimization depends on edge ordering -> canonical keys, stable tie-breaking, best-result retention, and input-permutation regressions.
- Body graphs or partial-node clipping may retain fixed-size assumptions -> exercise root and Loop scopes, dynamic heights, and viewport/full-canvas equivalence.

## Migration Plan

Implement the marker prototype and route model first, then routing-aware projection, channel separation, bounded search, and composed rendering. Integrate unresolved-edge presentation and replace the midpoint limitation in `docs/observability.md`. Existing runners need no recompilation because descriptions and lifecycle protocols are unchanged. Reverting the renderer change restores the previous presentation without migrating workflow data.

## Research References

- [Orthogonal Connector Routing](https://users.monash.edu/~mwybrow/papers/wybrow-gd-2009.pdf): orthogonal visibility graph, A* route search, shared-path ordering, and separation constraints. Its optimality claim concerns individual route length/bends, not global crossing minimization.
- [libavoid routing parameters and options](https://www.adaptagrams.org/documentation/namespaceAvoid.html): obstacle clearance, nudging, and rerouting controls. The documented crossing/shared-path penalties include experimental options; they are not a non-overlap guarantee.
- [ELK Layered](https://eclipse.dev/elk/reference/algorithms/org-eclipse-elk-layered.html): layered orthogonal routing with ports, multi-edges, and edge/channel spacing controls.
- [PathFinder](https://janders.eecg.utoronto.ca/1387_2015/readings/pathfinder.pdf): current and historical congestion costs for iterative routing, retained as a possible later enhancement.
- [yFiles crossing styles](https://docs.yworks.com/yfiles-html/dguide/bridges-customizations/): bridge/gap styles and independent crossing-layer policies; terminal patterns require their own visual verification.

## Routing Measurements

Release fixtures on the local macOS host measured a 10,000-node chain with 9,999 edges in 33.1 ms, 29,997 conflict checks, no search, no unresolved edges, and approximately 27.0 MB maximum resident memory. The earlier row-only index exhausted two million conflict checks on that same fixture; interval-tree pruning removes that accidental quadratic work. A regression checks the work count rather than machine-dependent elapsed time.

The 256-node fork/join fixtures took approximately 1.3 ms with no unresolved edges. A 128-edge parallel data/control fixture took 0.39 ms. The 30-node, 300-edge dense cross-layer fixture took 13.9 ms and retained eight explicitly unresolved edges after two retries, demonstrating that the limits bound effort without claiming complete routing. Its 16,374 crossings approached the retained-crossing limit. These are fixture measurements, not a guarantee for arbitrary dense graphs.

Raw timing, maximum-resident-memory, and counter reports remain local in `target/edge-routing/measurements.csv` and per-fixture `.time` files. Reproduce with `nix develop --command cargo run --release -p mf-tui --example benchmark_routes -- <mode> <size>`, where modes are `chain`, `fan`, `join`, `parallel`, and `dense`.

## Renderer Evidence

Sparse and mixed crowded previews were exported through `graph_preview -- --dump [--crowded] [--body]` and inspected as Menlo renderings. The crowded fixture has 16 connections and 54 explicit crossings with no unresolved edges; its root and Loop-body text exports match exactly. Local evidence is `target/edge-routing/root.txt`, `root.png`, `crowded-root.txt`, `crowded-root.png`, and `crowded-body.txt`.

Renderer regressions compare full-canvas cells, including style and modifiers, against cropped markers, tall-node text, and Unicode labels at nonzero buffer origins. Separate tests verify that crossing strokes keep their own observation styles, unresolved routing remains independent of lifecycle completeness, and details remain reachable beyond 65,535 rendered rows.
