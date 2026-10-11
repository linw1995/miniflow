# Verbosity Levels

## Why

Users need richer execution diagnostics without making the default verbose output difficult to scan. Repeating `-v`
should progressively expose the metadata already available in the existing lifecycle pipeline.

## What Changes

- Count `-v`, clustered flags, and repeated `--verbose`, capping execution detail at three.
- Preserve current summaries at level one, add readable event/node/workflow metadata at level two, and serialize the
  complete existing `WireRecord` at level three.
- Forward the selected count through direct CLI execution, TUI launches, generated task runners, and streaming runners.
- Preserve consumed option-like input values, result stdout, OTLP delivery, telemetry-disabled support, and reminders.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-observability`: cumulative argument parsing and three local execution presentation levels.

## Impact

The new verbose count replaces the boolean in `RunnerArguments`, `RunOptions`, and observer verbosity entry points.
Quiet callers initialize it to `0`; level-one callers use `1`. These APIs belong to the same unmerged feature change.
Compilation uses positive verbosity to enable its existing build summary. Execution details use the selected level.
