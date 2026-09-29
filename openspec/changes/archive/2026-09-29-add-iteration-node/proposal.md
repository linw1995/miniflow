# Proposal

## Why

Flows can transform an array with one Code expression, but cannot run a multi-node body independently for every element or select how individual failures affect the collected result. The existing generated runner calls every outer node once, so iteration needs an explicit subgraph boundary.

## What Changes

- Add the structural `builtin.iteration` kind with an `items` array input, `results` array output, and a body graph selected by one result port.
- Expose the current element and zero-based index through the reserved `@iteration` body source.
- Support sequential and bounded parallel execution, input-order results, and terminate, continue-with-null, and remove-failed policies.
- Validate the body before installation and generate direct calls for its nodes inside a per-item execution closure.
- Keep the Iteration node as one outer lifecycle and terminal UI node, while exporting correlated item and body-node spans and logs under a separate instrumentation scope.

## Capabilities

### New Capabilities

- `iteration-execution`: Repeated scoped body execution and ordered result collection.

### Modified Capabilities

- `workflow-binary-compilation`: Plan, validate, and generate a standalone iteration body.
- `workflow-observability`: Observe the outer Iteration node without violating the one-lifecycle-per-node contract.

## Impact

`mf-runtime` gains iteration scheduling and item contexts. `mf-compiler` gains body parsing, validation, type propagation, normalization, and code generation. Workflow definitions retain schema version `2026-09-26`; ordinary plugins and existing definitions retain their current shape. An Iteration body uses the enclosing workflow's declared node packages.

Nested iterations, direct outer-context captures, and Answer-node streaming are outside this change.
