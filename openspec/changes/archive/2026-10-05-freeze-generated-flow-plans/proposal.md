## Why

Unified domain execution replaced generated dependency constants and fixed task calls with startup Flow construction and domain partitioning. Generated executables should use precompiled graph layouts while sharing the same scheduler as in-memory Flows.

## What Changes

- Emit immutable dependencies, indexed edges, execution orders, selected outputs, execution domains, and stream message ownership during generated-project compilation.
- Load linked provider registrations in the generated Cargo build script to resolve stream executor boundaries without built-in kind assumptions.
- Bind fresh executors to borrowed plans at launch; preserve node initialization, runtime input validation, worker limits, FIFO, and scoped execution.
- Keep dynamic construction for in-memory callers and reuse the same runtime plan representation and schedulers.

## Capabilities

### Modified Capabilities

- `workflow-runtime-execution`: generated execution uses compiled immutable plans.
- `workflow-binary-compilation`: generated project build resolves providers before emitting plans.

## Impact

Runtime plan storage, compiler code generation, generated project manifests/build scripts, existing generated-runner tests, and documentation.
