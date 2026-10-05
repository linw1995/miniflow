## Why

Workflow graph builders and their construction errors currently live in the execution crate. This obscures the construction/execution boundary and makes moving errors alone imply an invalid reverse dependency.

## What Changes

- Move workflow graph construction, domain partitioning, stream validation, and workflow construction errors to `mf-compiler`.
- Keep immutable executable plans, executor contracts, node state, and scheduling in `mf-runtime`.
- Bind generated static plans without rebuilding their graphs; preserve the existing provider initialization contract.
- Retain the ablation cleanup and move construction tests to their owning crate.

## Capabilities

### Modified Capabilities

- `node-preparation`: enforce the compiler/runtime crate ownership boundary.

## Impact

Public graph construction APIs move to the compiler. Runtime plan binding remains independent of the compiler, with no reverse dependency or compatibility facade.
