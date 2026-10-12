## Context

The graph already uses Unicode single/double lines and orthogonal routes. Its vertical cost comes
from six-row minimum boxes, six-row attachment intervals, wide component gaps, and square crossing
reservations. Compact rendering must preserve edge identity, line kind, observation styling, and
safe geometry across root and Loop scopes.

## Decisions

### Compact first, widen only when it improves routing

Start with four-row boxes and four-row attachment spacing. Align one-to-one attachments; stagger
fan/join arrivals by half the attachment interval. Round sibling strides to the attachment grid
with at least two blank rows and use an additional four-row component gap.

Use existing channel allocation and routing. A congested compact layout may attempt six-row
attachment spacing once. Both attempts share geometry, conflict, total expansion, retry, and
retained-crossing budgets. Keep the wider result only when it resolves more connections, and
aggregate work counters even when it is rejected. Preserve the compact result when the remaining
budget cannot admit another attempt.

A twelve-node mixed fixture exposed a real regression: compact-only planning left unresolved
connections that the prior layout could route. Widening recovers those connections without changing
ordinary compact fixtures. This bounded second attempt is retained because the regression test
fails without it and the budget tests fail if its limits reset.

### Match crossing reservations to their rendered cells

Keep the upper stroke continuous and blank one lower-edge cell on either side of the crossing.
Preserve the lower edge's single or double line beyond that gap. Reserve a one-by-three or
three-by-one footprint according to orientation. Query that thin footprint once instead of
scanning its upper and lower boundaries separately.

### Retain only mechanisms with experimental evidence

Isolated ablations showed that removing the attachment grid, arrival stagger, reduced crossing
clearance, or producer-first routing creates unresolved connections in the deterministic corpus.
Restoring larger box, attachment, sibling, or component spacing increases vertical cost. Reports
and reproduction scripts remain under ignored `target/graph-ablation/`.

Changing same-kind crossing priority to producer order did not repair the expanded mixed fixture;
retain the existing stable-identity crossing policy. Add no layout configuration object, facade,
new dependency, or error type.

### Consolidate tests around observable behavior

Use one crossing matrix to verify actual single/double glyphs, blank gaps, styles, and both
orientations; reuse the viewport-crop test across all combinations. Remove the helper-shape test
and duplicate rendering assertions. Fold the dedicated name-order regression into the existing
permutation fixture, covering compact mixed, dense data-only, and widened mixed graphs. Keep the
linear-workflow test because mutations of minimum height or attachment alignment are otherwise
not rejected. Extend existing budget and root/Loop consistency tests to include widening.

Mutation checks reject removing widening, resetting scope budgets, removing grid/stagger/producer
priority or one-to-one alignment, and corrupting double strokes or blank gaps.

## Boundaries and tradeoffs

Keep the existing graph module and public planning/rendering entry points. The spacing-specific
constructor and row-stride helper are private implementation details. Existing Snafu errors and
typed source chains remain intact. Congested graphs may use more rows to retain traceable
connections; exhausted budgets still expose unresolved metadata instead of drawing unsafe routes.
Layout remains static after planning, independent of observation updates and viewport movement.
