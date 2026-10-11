## 1. Establish crossing presentation and route ownership

- [x] 1.1 Extend the graph preview with bridge and capped-underpass prototypes; compare both crossing orientations, all data/control pairs, and monochrome/status styles, then record the chosen glyph pattern, cell widths, footprint, clearance, and visual evidence in `design.md` and local `target/` artifacts.
- [x] 1.2 Add stable edge keys and stored polylines, labels, crossing ownership, and unresolved reasons within existing graph module boundaries; verify endpoint identity, distinct parallel edges, and unchanged observation-index mapping with focused graph tests.
- [x] 1.3 Add interval-based conflict classification for obstacles, collinear overlap, contacts, bends, perpendicular crossings, and marker footprints; verify each category with small geometric fixtures, including reverse segment direction and inclusive endpoints.

## 2. Allocate attachments and routing channels

- [x] 2.1 Replace modulo-four attachments with deterministic distinct rows and routing-aware node heights; verify more than four incoming/outgoing edges, mixed edge kinds, node disjointness, and title/detail rendering in dynamically sized boxes.
- [x] 2.2 Preserve layer/sibling ordering while reserving intermediate passage slots for long edges and updating canvas bounds; verify skipped layers, isolated components, empty graphs, and root/Loop layouts without treating waypoints as observed nodes.
- [x] 2.3 Implement interval track allocation, shared-region ordering, integer separation, and channel expansion; verify overlapping/disjoint intervals, horizontal access conflicts, and constrained port orders without collinear overlap or spurious junctions.
- [x] 2.4 Reserve control labels, arrowheads, and crossing footprints using terminal display widths; verify opaque names, wide characters, truncation, corner proximity, neighboring markers, and bounds after expansion.

## 3. Route exceptions within explicit budgets

- [x] 3.1 Build component-local sparse search geometry and direction-aware orthogonal A*; verify obstacle detours, bend-aware choice, legal crossings, and rejection of third-edge, corner, arrow, and label collisions.
- [x] 3.2 Add deterministic bounded local rip-up-and-reroute with best-result retention and validated outer detours; verify congested fixtures improve or report unresolved routes without accepting unsafe geometry.
- [x] 3.3 Enforce named per-search and scope-wide limits for preprocessing, search expansions, conflict state, retry rounds, and canvas arithmetic; verify injected low limits terminate predictably and retain all unresolved edge identities and typed reasons.
- [x] 3.4 Measure chains, wide forks/joins, parallel edges, dense cross-layer fixtures, and a sparse 10,000-node graph; select and document default budgets in `design.md` and `docs/observability.md`, keeping time/memory/work-count reports under ignored `target/`.

## 4. Compose and expose routed graphs

- [ ] 4.1 Render stored paths and explicit crossing records with stable layer priority and per-edge styles; verify both upper/lower orientations, mixed line kinds, monochrome readability, and produced/skipped/uncertain styles against the selected prototypes.
- [ ] 4.2 Clip composed geometry to the viewport without rerouting; verify cell symbols and styles match full-canvas crops across marker/bend/node boundaries, nonzero render origins, narrow viewports, panning, and resizing.
- [ ] 4.3 Show unresolved-routing warnings separately from observation integrity and expose endpoint/port/kind/reason details for incident nodes; verify low-budget fixtures retain inspectable connections and do not change workflow results or lifecycle completeness.
- [ ] 4.4 Replace the midpoint-routing limitation in `docs/observability.md` with the new routing behavior, crossing legend, geometry expansion, limits, and unresolved inspection; verify documented preview and navigation behavior against the implementation.

## 5. Validate scope and execution integration

- [ ] 5.1 Exercise data/control edge-array permutations, lifecycle updates, Loop pass selection, and stream invocation selection; verify equivalent descriptions and unchanged scopes retain node geometry, paths, and crossing layers while observation styles update.
- [ ] 5.2 Run focused graph tests and existing CLI TUI integration tests through `nix develop`; visually inspect representative sparse and crowded root/body graphs and keep screenshots or text captures under `target/`.
- [ ] 5.3 Run `nix develop --command prek install`, `nix develop --command prek -a`, and `nix flake check -L`; resolve failures and record the integrated validation results.
- [ ] 5.4 Run `openspec validate route-tui-edges-without-overlap --strict`; map every delta scenario to implementation or visual evidence before marking tasks complete.
