## Stored facts and derived views

`WorkflowManifest` is a version-tagged representation. `Compiled` (`2026-10-16`) contains the existing
`WorkflowDefinition`, root order, nested `LoopExecutionOrder` records, and `WorkflowInputSchema`. It does not store
`WorkflowDescription`, `WorkflowInterface`, or workflow identity. `Legacy` preserves the released `2026-10-07` wire
shape. The representation prevents constructing mismatched version/field combinations without an extra version enum.

The runtime borrows original data through `definition()`, `node_definition(id)`, and `input_schema()`. It derives owned
views through `description()`, `interface()`, and `workflow_id()`. Compiler and runtime share the same graph projection;
only the compiler computes root and nested execution orders. Node IDs and kinds are emitted in those frozen orders,
including the implicit Loop source. Interface version and identity are derived from the retained schema and definition.
The manifest version fixes these rules, including observation schemas 2 and 4 for startup-aware single and stream flows.
Validation reuses the computed identity instead of hashing the definition twice.

Input schema remains a stored fact: provider preparation declares port types, required flags, bindings, and stdin
ownership that definition JSON alone cannot supply. It remains subject to the existing startup drift comparison.
No provider configuration-publication field, parallel summary map, or configuration drift validator is required.

## Generation and consumers

Standalone generation reuses the already prepared input schema. Embedded-host manifest construction prepares once.
Generated runners use manifest helpers directly for observations, snapshot identity, and input binding. They neither
embed another complete plan JSON nor reconstruct a compiler plan just to inspect it. Build scripts still consume the
plan JSON, and generated constructors retain the literals needed for execution.

TUI and other readers use the same helper API for both formats. Graph projection neither invokes factories nor replans
the DAG. Framing, size bounds, unique-key decoding, graph validation, and input validation retain typed error sources.
The telemetry invalid-contract selector is public because the runtime now constructs the shared graph contract.

## Compatibility

Legacy graph/interface views remain supported; legacy definition lookup returns `None`. Current manifests require
all source facts and reject duplicated derived properties. The generated-project layout is `2026-10-16`.
Raw configuration includes static private literals; invocation inputs and environment values are not materialized.
The manifest derives `PartialEq` to match the original definition model. Rust struct-field callers migrate to variants
and helpers. Future protocol changes must preserve existing versioned derivation rules.
