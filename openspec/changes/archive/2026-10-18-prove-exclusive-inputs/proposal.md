## Why

Exclusive edge markers currently defer conflicting candidates to execution. Workflow authors need compilation to reject every group whose mutual exclusion cannot be proven.

## What Changes

- Declare a mutually exclusive output set in task node metadata; publish it for `builtin.if_else`.
- Prove candidate pairs exclusive using branch facts propagated through mandatory dependencies and intersected at alternative inputs.
- Reject unproven groups during compilation and programmatic executable construction, including nested scopes.
- Keep only a defensive runtime invariant for invalid node implementations or unchecked plans; remove runtime ambiguity as a supported workflow outcome.

## Capabilities

### Modified Capabilities

- `exclusive-input-bindings`: require static proof before executable construction.

## Impact

Runtime node metadata, core routing nodes, compiler preparation, generated runner validation, documentation, and regression tests change. No new workflow syntax is introduced. Previously accepted unproven graphs become compile errors.
