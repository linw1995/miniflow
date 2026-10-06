## Why

Task input and output validation stringify `TypeMismatch` errors. Callers lose the typed source even though diagnostics still identify the failed port.

## What Changes

- Preserve runtime port type mismatches with their node and port context.
- **BREAKING**: add `WorkflowRunError::OutputType` and report task input mismatches through the existing `InputType` variant.
- Keep regression assertions focused on the input and output validation boundaries after isolated ablation.

## Capabilities

### Modified Capabilities

- `typed-port-contracts`: preserve typed runtime port validation sources.

## Impact

Changes affect runtime error matching and source inspection. The runtime continues to reject invalid inputs before invocation and invalid outputs before publication.
