## Context

The session already owns a selected node index for Details and scope navigation. Graph rendering receives observation data and viewport offsets, but previously received no selection.

## Decisions

### Use the existing selection

Pass the current scope's selected index through the existing `GraphView` builder. Keep selection optional so callers without focus retain ordinary rendering. Root and Loop views use the same rendering path and require no additional state.

Root snapshots and graph nodes follow execution order; Loop observations are materialized in layout order. The existing selection index therefore identifies the same node in the graph and Details.

### Use one independent focus cue

Apply `REVERSED` to the selected title's existing style. Preserve its lifecycle marker, status color, and existing modifiers. Borders, detail lines, connections, and layout continue to use their existing rendering.

The rendering ablation matrix covers both scopes, all eight lifecycle statuses, selection movement, unrelated cells, and clipping. Title reversal and alternate borders each provide visible focus; reversal requires fewer changed lines. Additional bold styling is redundant, and bold alone cannot distinguish a focused Running node. Retain title reversal and remove the alternate border glyphs and extra bold styling.

### Reuse a behavioral regression test

Extend the existing running-node rendering test to select source, router, and source again. Check the selected title's focus cue, retained text and colors, and restoration of the previous title. Remove the separate status-matrix test and its full-box scan; the experimental matrix remains local.

## Boundaries

Use the existing public graph module and builder pattern. Add no module, dependency, error type, navigation state, or automatic panning policy. Focus changes presentation independently of execution state and geometry.
