## ADDED Requirements

### Requirement: Display parallel running nodes in local PiP graphs

When two or more nodes run concurrently, the ordinary execution view and chat Flow execution panel
SHALL show local PiP graphs for the running nodes that fit. PiPs SHALL use precomputed expanded
layouts and the current connection view, without changing main-graph geometry, navigation, or
lifecycle state. The chat panel SHALL also show a PiP for a single running node in a folded body
alongside its root overview.

#### Scenario: Observe two parallel branches

- **WHEN** two different nodes are Running in the current scope
- **THEN** a separate local PiP graph for each node expands into its slot
- **AND** each PiP centers on and highlights its node without changing the main graph selection or viewport

#### Scenario: Adapt to nodes and terminal size

- **WHEN** the running-node count changes or the terminal is resized
- **THEN** the next frame recomputes the PiP grid and visible count from both inputs
- **AND** all running nodes appear when the available grid can contain them
- **AND** any running nodes that cannot fit are represented by an explicit overflow count

#### Scenario: Finish a parallel node

- **WHEN** fewer than two display nodes remain Running
- **THEN** the ordinary execution view briefly retains the last visible PiP graphs as recent activity before they retract
- **AND** completed nodes immediately take on their ordinary main-graph lifecycle presentation

#### Scenario: Replace a departing PiP

- **WHEN** a previously visible node no longer qualifies for a PiP in either view
- **THEN** its PiP briefly shows its last active graph as recent activity and then retracts
- **AND** a retained PiP yields its slot if a running node needs the space

#### Scenario: Inspect a collapsed cluster or body scope

- **WHEN** running source nodes are hidden by a collapsed cluster or a folded Loop, Iteration, or Call body
- **THEN** each running source node has its own PiP in the ordinary execution view and chat Flow execution panel
- **AND** body PiP graphs use only the selected body frame's observations
- **AND** the visible main graph and chat root overview retain their folded presentation

#### Scenario: Observe one nested Call node in chat

- **WHEN** the chat Flow execution panel has exactly one Running node inside a folded `workflow.call` body
- **THEN** the panel shows a PiP centered on and highlighting that node
- **AND** the panel's root overview remains folded
