## ADDED Requirements

### Requirement: Keep routed connections individually traceable

The graph SHALL retain every described data and control connection. Completed routes MUST avoid unrelated node boxes, labels, arrowheads, and occupied attachment cells. Distinct connections MUST NOT share collinear runs or appear joined at incidental contacts. Perpendicular crossings SHALL remain distinguishable from connections.

#### Scenario: Separate parallel data and control edges

- **WHEN** data and control connections link the same pair of nodes
- **THEN** their attachments and routes remain separately traceable with the existing line-kind distinctions

#### Scenario: Route around an intervening node

- **WHEN** a long connection spans multiple layers containing unrelated node boxes
- **THEN** its completed route passes through available space without crossing those boxes or their contents

#### Scenario: Preserve branch labels and arrows

- **WHEN** neighboring routes pass near a control label or a target arrowhead
- **THEN** neither the connection geometry nor its crossing markers overwrite that label or arrowhead

### Requirement: Expand geometry to separate crowded connections

The graph SHALL allocate distinct attachments for incident edges and enlarge node boxes or routing channels when their existing capacity is insufficient. Expansion SHALL preserve the flow direction, keep nodes disjoint, and update the pannable canvas bounds. Diagram dimensions SHALL remain subject to explicit resource limits.

#### Scenario: Exceed four incident edges

- **WHEN** a node has more than four incoming or outgoing connections
- **THEN** its attachment positions do not wrap onto positions occupied by other connections

#### Scenario: Exceed the initial channel capacity

- **WHEN** a layer gap lacks sufficient tracks to separate its routes within the supported resource limits
- **THEN** the gap expands and all completed routes remain inside the updated canvas bounds

### Requirement: Identify upper and lower edges at crossings

Each displayed perpendicular crossing SHALL identify one upper and one lower edge using an explicit marker readable without color. The marker MUST NOT resemble a junction or an unexplained missing segment. Crossings SHALL occur only within straight segments with sufficient marker clearance, excluding corners, attachments, labels, arrowheads, and other markers.

#### Scenario: Display both crossing orientations

- **WHEN** fixtures contain a horizontal-over-vertical crossing and a vertical-over-horizontal crossing
- **THEN** each marker visibly identifies the continuous connections and their different upper/lower relationships in monochrome

#### Scenario: Move a crowded crossing

- **WHEN** a proposed crossing is adjacent to a bend, arrowhead, label, or another crossing marker
- **THEN** routing separates the conflicting features or explicitly reports the connection as unresolved instead of drawing an ambiguous marker

### Requirement: Keep graph geometry independent of observations and viewport

For an unchanged described scope, node geometry, edge paths, and crossing layers SHALL remain stable across lifecycle updates, elapsed-time updates, panning, and viewport resizing. Reordering the edge arrays while preserving the same nodes, execution order, and connections SHALL produce the same routed geometry. Clipping SHALL preserve the visible portion of the full diagram.

#### Scenario: Change execution state at a crossing

- **WHEN** a pending edge becomes produced, skipped, or uncertain
- **THEN** its observation style changes without changing the path, marker shape, or upper/lower assignment

#### Scenario: Pan across a partially visible marker

- **WHEN** a viewport clips part of a crossing marker
- **THEN** its visible cells match the same region of a full-canvas rendering, including style

#### Scenario: Reorder connection descriptions

- **WHEN** equivalent descriptions list their data and control connections in different orders
- **THEN** their node boxes, edge paths, and crossing layer assignments match

### Requirement: Bound routing effort without hiding connections

Routing SHALL use explicit limits for search work, retained routing state, retry rounds, and canvas dimensions. If a connection cannot be safely routed within these limits, the UI SHALL visibly report unresolved routing and retain its endpoints, port names, and kind for inspection. It MUST NOT silently omit the connection or change workflow execution or observation integrity because of a routing limitation.

#### Scenario: Exhaust a routing budget

- **WHEN** a congested fixture exhausts a configured routing limit
- **THEN** routing terminates, the graph reports unresolved connections with inspectable identities, and no colliding path is presented as complete

#### Scenario: Inspect an unresolved connection

- **WHEN** the user inspects a node incident to an unresolved connection
- **THEN** its connection details identify the source, destination, connected ports, kind, and routing limitation

### Requirement: Apply routing behavior consistently to described scopes

The root graph and each inspectable Loop body SHALL use the same avoidance, overlap separation, crossing, and routing-limit behavior. Scope navigation SHALL preserve each scope's established geometry independently of the selected execution pass or stream invocation.

#### Scenario: Inspect multiple passes of a body

- **WHEN** the user switches between observed passes of the same Loop body
- **THEN** the body retains its routes and crossing layers while displaying the selected pass's observation state
