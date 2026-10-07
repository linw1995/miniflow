# Proposal

## Why

Iteration has one fixed input but still declares and decodes it manually. Adopt the shared typed input contract while retaining its public dynamic task entry point and body-dependent outputs.

## What Changes

- Define and export `IterationInputs` with a required shared `items` value.
- Prepare the registered provider as a typed task and derive its input metadata from the struct.
- Reuse one runtime typed execution function for both the adapter and the retained dynamic task implementation.
- Preserve array/map iteration, shared payloads, scheduling, result types, and failure policies.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `iteration-execution`: Expose struct-defined iteration inputs and consistent input decoding across public typed and dynamic task calls.

## Impact

Changes affect runtime task dispatch helpers, the Iteration provider's input interface and preparation, and provider documentation. Valid existing dynamic calls remain supported. Missing or unknown direct input bindings use the runtime input decode error; supplied scalar values retain Iteration's array-or-object domain diagnostic.
