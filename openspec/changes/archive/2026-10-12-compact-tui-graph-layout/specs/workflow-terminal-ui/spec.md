## ADDED Requirements

### Requirement: Compact graph rows without losing connections

The graph SHALL use four-row minimum boxes containing borders, titles, and detail lines. It SHALL
grow boxes to keep distinct attachments inside their borders, reduce vertical separation where safe,
and align one-to-one connections. Compact layouts MUST preserve node disjointness, connection
identity, routing limits, and explicit unresolved-route reporting.

#### Scenario: Display a linear workflow compactly

- **WHEN** a workflow consists of two nodes joined by one connection
- **THEN** the title and detail lines remain visible and the connection uses aligned rows without a vertical detour

#### Scenario: Compact crowded incident edges

- **WHEN** a node has several incoming or outgoing data/control connections
- **THEN** the box grows to contain distinct attachments and completed routes remain independently traceable without overwriting borders, labels, or arrows

#### Scenario: Preserve routes with opaque node names

- **WHEN** downstream node names sort before their producers
- **THEN** the compact layout retains safe routing independent of that lexical ordering

### Requirement: Share routing budgets when widening compact geometry

When compact geometry prevents safe routing, planning MAY try wider attachment separation once
within the remaining scope-wide budgets. It SHALL select the candidate with fewer unresolved
connections and report both attempts' work. Limits MUST NOT reset, and their combined retained
crossing storage MUST remain bounded.

#### Scenario: Recover a congested compact layout

- **WHEN** a mixed data/control graph cannot route all connections at compact attachment spacing
- **THEN** planning may widen spacing within its remaining budgets and select that geometry only if it resolves more connections

#### Scenario: Exhaust the budget before widening

- **WHEN** the compact attempt consumes the scope's geometry or conflict budget
- **THEN** planning retains its safe completed routes and unresolved metadata without resetting limits for another attempt

## MODIFIED Requirements

### Requirement: Identify upper and lower edges at crossings

Crossings SHALL be readable without color and show a continuous upper edge with a blank cell on
each side of the lower edge. Lines outside the gap SHALL retain their single or double strokes.
Routing SHALL reserve the actual one-row or one-column marker footprint and sufficient clearance
from bends, attachments, labels, arrows, and other markers. Markers MUST NOT imply a junction or
an unexplained broken connection.

#### Scenario: Display both crossing orientations

- **WHEN** fixtures contain a horizontal-over-vertical crossing and a vertical-over-horizontal crossing
- **THEN** each marker visibly identifies the continuous upper connection and interrupted lower connection in monochrome while retaining the data/control line distinctions

#### Scenario: Move a crowded crossing

- **WHEN** a proposed crossing is adjacent to a bend, arrowhead, label, or another crossing marker
- **THEN** routing separates the conflicting features or explicitly reports the connection as unresolved instead of drawing an ambiguous marker

#### Scenario: Reserve only rendered marker cells

- **WHEN** an unrelated route lies beside a crossing but outside its one-row or one-column marker footprint
- **THEN** the marker does not overwrite that route or reserve unused corner cells
