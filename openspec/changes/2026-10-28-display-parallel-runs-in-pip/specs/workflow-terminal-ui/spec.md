### Requirement: Display parallel running nodes in local PiP graphs

When at least two different display nodes are Running in the current graph scope, the ordinary TUI execution view and chat
Flow execution panel SHALL present a picture-in-picture local graph for each running node that fits the available area.
Each PiP SHALL use the current connection view's observations and a precomputed fully expanded layout with an independent
viewport centered on its running node. The main graph selection, pan state, navigation, folding, geometry, and lifecycle
state SHALL remain independent of PiP rendering.

The visible PiP count and grid dimensions SHALL adapt to both the running-node count and available terminal width and height. The view SHALL display all running nodes when they fit and SHALL report the hidden count when space is insufficient. The chat Flow execution panel SHALL retain its compact root overview alongside the PiP grid.

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

- **WHEN** running source nodes are hidden by a collapsed cluster or a folded Loop, Iteration, or Call body
- **THEN** each running source node has its own PiP in the ordinary execution view and chat Flow execution panel
- **AND** body PiP graphs use only the selected body frame's observations
- **AND** the visible main graph and chat root overview retain their folded presentation
