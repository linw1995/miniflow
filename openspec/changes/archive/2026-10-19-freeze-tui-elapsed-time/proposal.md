## Why

The TUI keeps advancing elapsed time after the workflow process has exited, so inspecting a completed run changes its displayed duration.

## What Changes

- Keep elapsed time advancing while a local run is active and freeze it at the observed process exit time once completion is published.
- Reuse the existing shared completion state for all observation handles.
- Retain one focused timing regression and the existing process-outcome integration coverage.

## Capabilities

### Modified Capabilities

- `workflow-terminal-ui`: keep the final run duration stable during inspection, including runs without terminal telemetry.

## Impact

Changes affect local run observation and its regression coverage in `mf-tui`. No dependencies or public API signatures change.
