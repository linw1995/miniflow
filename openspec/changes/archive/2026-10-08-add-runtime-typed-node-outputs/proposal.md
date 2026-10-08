# Proposal

## Why

Typed tasks currently receive owned input structs but still construct output maps and declare output ports separately. Struct-defined outputs make field names, presence, types, and runtime encoding agree at the provider boundary.

## What Changes

- Add owned output declarations and strict encoding for the same scalar and collection types as typed inputs.
- **BREAKING**: Typed task providers declare an output struct and return typed results; their factories no longer supply separate output ports.
- Preserve dynamic task results, explicit skips, loop summaries, shared payloads, and typed error chains.
- Preserve Iteration's body-dependent output precision through checked instance refinements.
- Migrate Identity and Iteration and document the provider migration.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: Define output structs, presence semantics, strict encoding, and shared-value/error contracts.
- `node-preparation`: Derive both task directions, validate output refinements, and encode typed results in the runtime adapter.
- `iteration-execution`: Expose collected typed results while preserving the dynamic task interface and body-derived types.

## Impact

Changes affect the runtime and derive APIs, typed task providers, existing compiler fixtures, and node-development documentation. Workflow definitions, dynamic task/event/stream contracts, dependency versions, and manifest framing remain unchanged.
