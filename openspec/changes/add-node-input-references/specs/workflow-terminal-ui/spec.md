## ADDED Requirements

### Requirement: Select explicit connection views

The graph SHALL offer Execution, Dependencies, and Ports views. Default Execution SHALL route node-success edges and
output gates and report
hidden data counts. Dependencies SHALL add data bundles by source, target, and ordinary/exclusive kind. Ports SHALL
route all original connections. Success edges and output gates SHALL stay individually identifiable. All nodes and
mappings SHALL remain
inspectable, including for existing compatible executables without recompilation.

#### Scenario: Inspect a parameter-heavy node pair

- **WHEN** eighteen ordinary data edges connect the same source and consumer
- **THEN** Execution reports them as hidden, Dependencies displays one counted bundle with all eighteen inspectable mappings, and Ports displays individual mappings

#### Scenario: Keep control and exclusive identities

- **WHEN** one node pair has success edges, control gates, ordinary data bindings, and multiple exclusive-input candidates
- **THEN** success edges and control gates remain separate, data bundles distinguish ordinary and exclusive kinds, and details retain each target input and candidate-group identity

#### Scenario: Inspect a data-only workflow

- **WHEN** a scope has data dependencies but no execution connections
- **THEN** Execution retains dependency-based node placement and explicitly indicates hidden data dependencies without inventing an execution chain

### Requirement: Inspect bindings independently of visible routes

Selected-node details SHALL retain incoming input bindings, outgoing consumers, source/target ports, and
ordinary/exclusive policy in every connection view. Available observation evidence MAY identify selected sources and
skip causes; incomplete evidence MUST remain unknown rather than inferred from aggregate producer success. Existing
redaction and observation-integrity rules SHALL apply. Selection alone MUST NOT change route membership or trigger
layout recomputation.

#### Scenario: Inspect a hidden exclusive group

- **WHEN** a consumer is selected in Execution view
- **THEN** its details show all exclusive sources and any available selected-source evidence without exposing secret values

#### Scenario: Retain uncertainty

- **WHEN** observations do not establish which candidate supplied an input
- **THEN** the detail view reports unknown selection instead of choosing a producer from its aggregate lifecycle state

### Requirement: Bound presentation state across views

Topology SHALL derive from all described semantic dependencies. Attachment counts, node growth, routes, and route-search
work SHALL derive only from the selected view's visible connections. Layouts SHALL be built lazily and cached under a
shared bounded retained-state allowance across views and scopes. Existing per-layout routing and retry limits SHALL
apply. Hidden mappings SHALL be distinguished from unresolved routes; unresolved bundles SHALL retain every member
identity for inspection.

#### Scenario: Hide a large parameter fan-out

- **WHEN** Execution hides data mappings incident to a node
- **THEN** those mappings do not reserve attachments, enlarge its box, or consume route-search work

#### Scenario: Exhaust routing for an aggregate

- **WHEN** a Dependencies bundle cannot be routed within limits
- **THEN** the view reports an unresolved bundle with its complete member endpoints and does not count intentionally hidden mappings as routing failures

#### Scenario: Switch views across many bodies

- **WHEN** users inspect multiple scopes and connection views
- **THEN** layout-cache retention remains bounded and evicted layouts rebuild deterministically under routing limits

### Requirement: Share connection navigation across graph hosts

Ordinary runs and chat graph inspection SHALL share connection-view navigation and help. The `e` key SHALL cycle views
only when graph navigation owns input, without intercepting chat prompt editing. View switches SHALL preserve the
selected node by identity and restore the saved viewport for each scope/view. State-dependent active-path filtering
SHALL NOT be implied by a connection view.

#### Scenario: Switch connections while inspecting a body

- **WHEN** a user cycles views and later returns to the same body/view
- **THEN** selection and saved viewport are preserved while the established geometry remains deterministic

#### Scenario: Type in chat

- **WHEN** the chat prompt editor owns keyboard input and the user types `e`
- **THEN** the character edits the prompt without changing the graph view

### Requirement: Distinguish execution prerequisites from data mappings

Every view SHALL distinguish node-success edges from output gates and data bindings, including in monochrome and
details. Node-success edges SHALL identify source success without fabricating ports or input values. Details SHALL
retain success prerequisites and typed skip causes. Bundle styles SHALL remain mixed or unknown when member evidence
differs or is incomplete.

#### Scenario: Inspect both execution prerequisites between one node pair

- **WHEN** a pair has both a node-success edge and an output gate
- **THEN** both remain separately identifiable with success and source-output semantics

#### Scenario: Retain a mixed bundle outcome

- **WHEN** members of a data bundle have different or incomplete observations
- **THEN** the bundle does not falsely display uniform success or a single selected source

## MODIFIED Requirements

### Requirement: Keep routed connections individually traceable

The graph SHALL retain every described data binding, output gate, and node-success connection in its semantic model and
inspection details. Routing
SHALL operate on the selected connection view, where an explicit data bundle is one visible connection with inspectable
members. Completed routes MUST avoid unrelated node boxes, labels, arrowheads, and occupied attachment cells. Distinct
visible connections MUST NOT share collinear runs or appear joined at incidental contacts. Perpendicular crossings SHALL
remain distinguishable from connections.

#### Scenario: Separate parallel data and control edges

- **WHEN** data and control connections link the same pair of nodes in Ports view
- **THEN** their attachments and routes remain separately traceable with the existing line-kind distinctions

#### Scenario: Route around an intervening node

- **WHEN** a long visible connection spans multiple layers containing unrelated node boxes
- **THEN** its completed route passes through available space without crossing those boxes or their contents

#### Scenario: Preserve branch labels and arrows

- **WHEN** neighboring routes pass near a control label or a target arrowhead
- **THEN** neither the connection geometry nor its crossing markers overwrite that label or arrowhead

### Requirement: Expand geometry to separate crowded connections

The graph SHALL allocate distinct attachments for incident visible connections and enlarge node boxes or routing
channels when their existing capacity is insufficient. Hidden data mappings SHALL NOT reserve attachments; an aggregate
bundle SHALL reserve one attachment per endpoint. Expansion SHALL preserve the flow direction, keep nodes disjoint, and
update the pannable canvas bounds. Diagram dimensions SHALL remain subject to explicit resource limits.

#### Scenario: Exceed four incident edges

- **WHEN** a node has more than four incoming or outgoing visible connections
- **THEN** its attachment positions do not wrap onto positions occupied by other connections

#### Scenario: Exceed the initial channel capacity

- **WHEN** a layer gap lacks sufficient tracks to separate its visible routes within the supported resource limits
- **THEN** the gap expands and all completed routes remain inside the updated canvas bounds

### Requirement: Keep graph geometry independent of observations and viewport

For an unchanged described scope and connection view, node geometry, edge paths, and crossing layers SHALL remain stable
across selection, lifecycle updates, elapsed-time updates, panning, selected execution passes, and viewport resizing.
Reordering edge arrays while preserving the same nodes, execution order, and connections SHALL produce the same routed
geometry and bundle membership. Explicit connection-view switches MAY change geometry; returning to a view SHALL
reproduce its established geometry. Clipping SHALL preserve the visible portion of the full diagram.

#### Scenario: Change execution state at a crossing

- **WHEN** a pending visible connection becomes produced, skipped, or uncertain
- **THEN** its observation style changes without changing the path, marker shape, or upper/lower assignment

#### Scenario: Pan across a partially visible marker

- **WHEN** a viewport clips part of a crossing marker
- **THEN** its visible cells match the same region of a full-canvas rendering, including style

#### Scenario: Reorder connection descriptions

- **WHEN** equivalent descriptions list their data and control connections in different orders
- **THEN** their node boxes, bundle membership, edge paths, and crossing layer assignments match within the same connection view

### Requirement: Bound routing effort without hiding connections

Routing SHALL use explicit limits for search work, retained routing state, retry rounds, and canvas dimensions. If a
visible connection cannot be safely routed within these limits, the UI SHALL visibly report unresolved routing and
retain its endpoints, port names, kind, and bundle members where applicable for inspection. It MUST NOT silently omit
that connection or change workflow execution or observation integrity because of a routing limitation. Intentional view
filtering SHALL be labelled separately and SHALL NOT count as routing failure.

#### Scenario: Exhaust a routing budget

- **WHEN** a congested fixture exhausts a configured routing limit
- **THEN** routing terminates, the graph reports unresolved visible connections with inspectable identities, and no colliding path is presented as complete

#### Scenario: Inspect an unresolved connection

- **WHEN** the user inspects a node incident to an unresolved connection
- **THEN** its connection details identify the source, destination, connected ports, kind, bundle members where applicable, and routing limitation

### Requirement: Compact graph rows without losing connections

The graph SHALL use four-row minimum boxes containing borders, titles, and detail lines. It SHALL grow boxes to keep
distinct visible attachments inside their borders, reduce vertical separation where safe, and align one-to-one visible
connections. Compact layouts MUST preserve node disjointness, visible connection identity, inspectable hidden and
bundled mappings, routing limits, and explicit unresolved-route reporting.

#### Scenario: Display a linear workflow compactly

- **WHEN** a workflow consists of two nodes joined by one visible connection
- **THEN** the title and detail lines remain visible and the connection uses aligned rows without a vertical detour

#### Scenario: Compact crowded incident edges

- **WHEN** a node has several incoming or outgoing visible data/control connections
- **THEN** the box grows to contain distinct attachments and completed routes remain independently traceable without overwriting borders, labels, or arrows

#### Scenario: Preserve routes with opaque node names

- **WHEN** downstream node names sort before their producers
- **THEN** the compact layout retains safe routing independent of that lexical ordering
