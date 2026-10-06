## Why

Loop structure and compiler type validation convert typed failures into diagnostic strings. Callers lose the original error types and cannot inspect their source chains.

## What Changes

- Preserve typed configuration, graph, derivation, depth, and known-value errors with their existing diagnostic context.
- **BREAKING**: add source-bearing `WorkflowCompileError` variants for failures previously reported as message-only errors.
- Remove redundant Loop node checks, unnecessary error boxing, and duplicated test cases after isolated ablation.

## Capabilities

### Modified Capabilities

- `typed-port-contracts`: preserve typed compiler validation failures.
- `workflow-loop-execution`: preserve typed Loop preparation failures and scope paths.

## Impact

Changes affect compiler diagnostics and error matching. Workflow execution, node ownership, and dependency versions remain unchanged.
