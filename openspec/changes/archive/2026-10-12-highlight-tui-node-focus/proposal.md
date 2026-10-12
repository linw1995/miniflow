## Why

Node selection updates Details without identifying the same node in the graph. Users need a visible focus indicator while inspecting root and Loop scopes.

## What Changes

- Pass the current selected node index to the graph widget.
- Reverse the selected node's title while preserving lifecycle markers and status colors.
- Reuse the existing graph rendering test for focus movement and restoration.

## Capabilities

### Modified Capabilities

- `workflow-terminal-ui`: identify the node selected for inspection in the displayed scope.

## Impact

Changes affect the existing graph widget, session rendering, and graph test. Experimental scripts and reports stay under ignored `target/focus-ablation/`.
