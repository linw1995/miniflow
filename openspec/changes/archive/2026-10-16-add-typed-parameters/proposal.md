## Why

Workflows need typed runtime configuration from runner arguments or named environment
variables, with source-level secret marking. A dedicated parameter node provides
stable values for downstream edges and future configuration templating in #145.

## What Changes

- Add `builtin.parameters` with one typed input/output pair per configured parameter.
- Resolve explicit arguments, environment fallbacks, and defaults before node execution.
- Mark secret inputs before snapshots and preserve sensitivity through forwarding.
- Freeze binding rules in startup interfaces and isolated task contracts.

## Capabilities

### New Capabilities

- `parameters-node-execution`: typed parameter declarations and forwarding.

### Modified Capabilities

- `workflow-inputs`: startup source resolution, secret marking, and binding contract validation.

## Impact

Core nodes, shared runtime input binding, compiler interface generation, focused
regression tests, and documentation. No dependency is added. Configuration templates
remain outside this change; #145 remains open.

## Non-goals

No process-wide environment enumeration, optional parameter policy, implicit type
coercion, runtime schema inference, or configuration template syntax.
