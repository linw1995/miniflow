## Stored facts and derived views

`WorkflowManifest` is a struct containing the `2026-10-16` format version, existing `WorkflowDefinition`, root order,
nested `LoopExecutionOrder` records, and `WorkflowInputSchema`. It does not store `WorkflowDescription`,
`WorkflowInterface`, or workflow identity. The version type accepts only the current format.

The runtime exposes definition and input schema as fields and borrows original nodes through `node_definition(id)`.
It derives owned views through `description()`, `interface()`, and `workflow_id()`. Compiler and runtime share the same
graph projection; only the compiler computes root and nested execution orders. Node IDs and kinds follow those orders,
including the implicit Loop source. Interface version and identity derive from the retained schema and definition.
Execution descriptions store only the execution mode; the telemetry helper derives the event schema from description
version and mode. The manifest does not maintain an independent mapping of historical event protocol numbers.
Validation reuses the computed identity instead of hashing the definition twice.

Input schema remains a stored fact: provider preparation declares port types, required flags, bindings, and stdin
ownership that definition JSON alone cannot supply. It remains subject to the existing startup drift comparison.
No provider configuration-publication field, parallel summary map, or configuration drift validator is required.

## Generation and consumers

Standalone generation reuses the already prepared input schema. Embedded-host manifest construction prepares once.
Generated runners use manifest helpers directly for observations, snapshot identity, and input binding. They neither
embed another complete plan JSON nor reconstruct a compiler plan just to inspect it. Build scripts still consume the
plan JSON, and generated constructors retain the literals needed for execution.

TUI and other readers consume the current manifest format. Graph projection neither invokes factories nor replans
the DAG. Framing, size bounds, unique-key decoding, graph validation, and input validation retain typed error sources.
The telemetry invalid-contract selector is public because the runtime now constructs the shared graph contract.

## Pre-release scope

There is no released manifest contract to preserve. Older manifest layouts require recompilation; definition is always
present, so no optional-definition accessor, compatibility variant, or missing-definition error is needed. Current
manifests require all source facts and reject duplicated derived properties. The generated-project layout is
`2026-10-16`. Existing Flow language versions retain their execution semantics.
Raw configuration includes static private literals; invocation inputs and environment values are not materialized.
The manifest derives `PartialEq` to match the original definition model. Rust callers use fields for stored facts and
helpers for derived views.
