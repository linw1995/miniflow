## Why

Known-value compilation and Loop preparation still manually construct Snafu errors. Producer panic handling converts a synthesized stream error to text, preventing callers from inspecting its original type.

## What Changes

- Use generated selectors for known-value type conflicts and Loop validation.
- Preserve producer panic errors through the existing `From<StreamError>` conversion.
- Extend the existing producer regression with a typed-source assertion.

## Capabilities

### Modified Capabilities

- `stream-node-execution`: retain the synthesized stream error in producer panic failures.

## Impact

Changes affect compiler, runtime, and core-node error handling. The Loop factory uses one newly exported runtime selector. Workflow schemas and execution results remain unchanged.
