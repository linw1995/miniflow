## Why

The main graph follows the latest node event, so parallel running nodes do not all retain a visible local focus. Users need to inspect parallel branches without repeatedly changing the main selection.

## What Changes

- Show a picture-in-picture local graph for each running node when at least two nodes run concurrently in the displayed scope.
- Size and fill a dynamic PiP grid from the running-node count and available terminal area, reporting only nodes that do not fit.
- Reuse the current layout and observations without changing graph geometry, lifecycle telemetry, navigation, or compact chat execution summaries.

## Non-goals

- Do not create independent layouts, interactive PiP focus, or repeated full-flow views.
- Do not change runtime execution, telemetry contracts, or graph routing.
