## Why

Mutually exclusive branches cannot currently feed one ordinary input: duplicate bindings are rejected, and any skipped dependency skips its consumer. Workflows need an explicit way to select the unique produced source without introducing a forwarding node or weakening ordinary activation rules.

## What Changes

- Add explicit `exclusive_inputs` groups to workflow and supported body definitions, each binding multiple candidate outputs to one existing input port.
- Resolve every candidate in the current frame; select exactly one produced value, propagate an all-skipped group, and reject multiple produced values or unexpected absence.
- Keep ordinary inputs and control dependencies conjunctive, including connected optional inputs.
- Validate all candidate dependencies, types, scopes, and message domains; preserve shared values and sensitivity metadata.
- Apply the same contract to in-memory and compiled execution, with input-source diagnostics and conservative typed-plan behavior.

## Non-goals

No `builtin.merge` kind, global OR activation, first-completion selection, cross-stream joining, implicit branch inference, multi-port transactional selection, or CI workflow migration.

## Capabilities

### New Capabilities

- `exclusive-input-bindings`: explicit per-input candidate selection, validation, execution, and diagnostics.

### Modified Capabilities

- `conditional-flow-execution`: apply activation and skip propagation to resolved logical inputs, retaining ordinary data and control dependency rules.

## Impact

Runtime definition and input-resolution contracts; compiler construction, inference, scheduling, and generated plans; loop and iteration body preparation; workflow descriptions and dependency diagnostics; examples and focused acceptance tests. No new provider or external dependency is required. Existing serialized definitions retain their behavior; low-level Rust definition and plan construction sites need updating for the additive fields.
