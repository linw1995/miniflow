## Why

The TUI currently draws each edge through a midpoint column directly into the terminal buffer. Crowded graphs can hide connections through shared segments, intervening nodes, labels, or later drawing operations, and unavoidable crossings do not identify which edge passes above the other.

## What Changes

- Plan orthogonal edge routes once per described scope, preserving the existing layered node arrangement and stable geometry during execution.
- Allocate distinct terminal attachment rows and integer channel tracks, expanding node and channel dimensions when needed instead of reusing occupied cells.
- Detect node, label, segment, corner, arrow, and crossing-marker conflicts before rendering; separate collinear overlaps and reduce perpendicular crossings.
- Use bounded obstacle-aware search and local rerouting for long edges and congested channels, with an explicit unresolved-routing presentation when resource limits prevent a complete result.
- Represent crossings explicitly with stable upper/lower edge identities and reserve space for a terminal crossing marker. Compare bridge and capped-underpass prototypes before choosing the final glyph pattern.
- Compose routes and crossing markers before viewport clipping, retaining data/control line distinctions and observation-driven colors.

## Capabilities

### Modified Capabilities

- `workflow-terminal-ui`: present individually traceable graph edges with obstacle avoidance, overlap separation, stable crossing layers, and bounded routing behavior.

## Impact

Implementation centers on `crates/mf-tui/src/graph.rs`, its graph tests and preview example, with routing diagnostics integrated into `crates/mf-tui/src/run.rs` if needed. Update `docs/observability.md` to replace the documented midpoint-routing limitation. Preserve existing module boundaries and observation APIs; introduce no runner protocol changes or external routing dependency in the initial implementation.
