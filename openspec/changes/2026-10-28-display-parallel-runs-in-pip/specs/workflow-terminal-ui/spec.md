### Requirement: Display parallel running nodes in local PiP graphs

When at least two different display nodes are Running in the current graph scope, the ordinary TUI execution view SHALL present a picture-in-picture local graph for each running node that fits the available area. Each PiP SHALL reuse the current scope's established layout and observations with an independent viewport centered on its running node. The main graph selection, pan state, navigation, geometry, and lifecycle state SHALL remain independent of PiP rendering.

The visible PiP count and grid dimensions SHALL adapt to both the running-node count and available terminal width and height. The view SHALL display all running nodes when they fit and SHALL report the hidden count when space is insufficient. Compact chat execution summaries SHALL retain their existing presentation.

#### Scenario: Observe two parallel branches

- **WHEN** two different nodes are Running in the current scope
- **THEN** the view shows a separate local PiP graph centered on and highlighting each node
- **AND** the main graph retains its existing selection and viewport

#### Scenario: Adapt to nodes and terminal size

- **WHEN** the running-node count changes or the terminal is resized
- **THEN** the next frame recomputes the PiP grid and visible count from both inputs
- **AND** all running nodes appear when the available grid can contain them
- **AND** any running nodes that cannot fit are represented by an explicit overflow count

#### Scenario: Finish a parallel node

- **WHEN** fewer than two display nodes remain Running
- **THEN** the parallel PiP grid is no longer shown
- **AND** completed nodes retain their ordinary main-graph lifecycle presentation

#### Scenario: Inspect a collapsed cluster or body scope

- **WHEN** running source nodes share a collapsed display cluster or belong to the selected Loop or Call body
- **THEN** each collapsed display node has at most one PiP
- **AND** body PiP graphs use only the selected body frame's observations
