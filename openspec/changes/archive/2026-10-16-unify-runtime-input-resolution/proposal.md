## Why

ExecutionContext stores startup arguments, resolved arguments, and initial-node identity
separately. Task and event execution also duplicate dependency assembly and input validation.
This makes binding lifecycle changes easy to apply inconsistently across execution paths.

## What Changes

- Own input state and startup binding together in ExecutionInputs.
- Share node input assembly and validation across task, generated, and event execution.
- Inject environment lookup into a pure startup resolver.
- Preserve dependency errors, skip precedence, typed transfers, secret metadata, and snapshots.

## Capabilities

### Modified Capabilities

- `workflow-inputs`: define binding invalidation and fork isolation.
- `workflow-runtime-execution`: define shared node input resolution across execution paths.

## Impact

Runtime implementation, public input APIs, regression tests, and specifications.
WorkflowInputSchema::resolve moves to resolve_workflow_inputs with explicit environment
lookup. Workflow formats, manifests, node contracts, and CLI behavior remain unchanged.
