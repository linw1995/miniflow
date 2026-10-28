## Implementation

- [x] Select running display nodes from current-scope observations and center independent PiP viewports on their existing graph cards.
- [x] Compute a dynamic grid from running-node count and available area, with overflow reporting and safe compact-terminal behavior.
- [x] Render PiP without changing main graph navigation, details, help, or telemetry state.

## Validation

- [x] Add unit coverage for running-count changes, resizing, overflow, completion, collapsed clusters, body observations, and tiny areas.
- [x] Add a PTY-backed runner scenario proving simultaneous PiP display and removal after completion.
- [x] Run the focused suites and repository checks, recording any validation limitations.
