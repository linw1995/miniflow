## Why

Standard generated runners compile unused convenience functions and report `dead_code` warnings for valid workflows. Those diagnostics describe generated implementation details rather than problems users can fix in their definitions.

## What Changes

- Generate only preparation and context execution for standard task runners, and only preparation for streaming runners.
- Preserve the existing general-purpose artifact generator for custom runners that use convenience entry points.
- Check every documented example for warning-free compilation and expected outputs; extend the existing telemetry-disabled runner test with the same diagnostic check.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-binary-compilation`: generate standard runner entry points on demand while preserving custom runner APIs and compiler diagnostics.

## Impact

Compiler artifact generation, generated project wiring, existing integration tests, and observability documentation. Workflow formats, runtime behavior, module visibility, and dependencies remain unchanged.
