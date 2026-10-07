## Why

Task dependency lookup, Loop assignment validation, and workflow output lookup stringify their errors. Callers lose typed causes and can only inspect formatted diagnostics.

## What Changes

- Use the existing `Dependency` variant for task dependency lookup failures.
- **BREAKING**: add `NodeExecutionError::LoopVariableType` and `WorkflowRunError::OutputSelection` with typed sources and contextual fields.
- Preserve dependency, execution, and workflow output selection attribution.
- Keep regression tests at the affected runtime boundaries.

## Capabilities

### Modified Capabilities

- `workflow-runtime-execution`: retain typed dependency and selected-output lookup causes.
- `workflow-loop-execution`: retain typed assignment validation causes.

## Impact

Downstream exhaustive matches on the public error enums require new arms. Missing values still fail before invocation or selection; invalid Loop writes are never staged.
