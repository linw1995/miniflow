## ADDED Requirements

### Requirement: Identify the node selected for inspection

The graph SHALL visibly identify the node selected for Details in the current root or Loop scope. Focus SHALL remain distinguishable for every displayed lifecycle state while preserving the node's status marker and colors. Selection changes SHALL move the focus indicator and restore the previous node's ordinary presentation without changing graph geometry or connection styling. Graph callers without a selection SHALL retain ordinary rendering.

#### Scenario: Move focus between nodes

- **WHEN** the user changes the selected node with Tab, Shift-Tab, j, or k
- **THEN** the graph highlights the newly selected node and restores the previous node's ordinary presentation

#### Scenario: Focus a running node

- **WHEN** the selected node is Running and another node is also Running
- **THEN** the selected node remains distinguishable while both retain their lifecycle markers and status colors

#### Scenario: Inspect a Loop body

- **WHEN** the user enters a Loop body or changes selection within it
- **THEN** the graph identifies the node selected for Details in that body using the same focus behavior as the root graph

#### Scenario: Pan across a focused title

- **WHEN** the viewport clips part of the selected node's title
- **THEN** its visible focus presentation matches the same region of the full graph rendering
