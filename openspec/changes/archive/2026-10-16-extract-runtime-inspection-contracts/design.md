## Decisions

`WorkflowManifest::definition` stores the compiler's existing `WorkflowDefinition`, including normalized node order
and unchanged configuration expressions. The runtime already owns this type, so inspection requires no compiler
planning dependency or parallel configuration representation. `node_definition(id)` returns a borrowed root node by
exact ID; unknown IDs and legacy manifests return `None`. It does not search nested scopes or invoke providers.

Manifest payload version `2026-10-16` requires a definition. Legacy `2026-10-07` omits it and remains readable.
Framing, size limits, duplicate-key decoding, and graph/interface validation are preserved. Inspection does not add
runtime configuration drift checks or another pass through provider preparation. The manifest carries full compiled
configuration, including static private literals, while graph descriptions and lifecycle records remain unchanged.

Both embedded-host and standalone generation populate the definition from the same compiled plan. Standalone runners
reconstruct the plan using that definition and the description's execution order, replacing runtime inclusion of the
complete `workflow-plan.json`. Build scripts still consume that file. Generated constructor literals remain part of
execution code generation. The generated-project layout advances to `2026-10-16`.

`ConfigTemplate::evaluate` and `decode_unique_json` reuse existing implementations through established runtime exports.
Model interpretation and configuration fingerprints remain caller concerns. No provider metadata field, configuration
collector, facade, dependency, or node-specific runtime error is needed.

## Compatibility

Old manifests remain usable for graph/interface inspection but cannot supply original definitions. The new manifest
field changes Rust struct literals; the manifest derives `PartialEq` to match the existing definition type. Newly
generated runners return a typed error if their manifest cannot supply a required definition.

## Validation

Cover borrowed lookup, unresolved values, exact IDs, legacy absence, version consistency, and compiler population.
Use existing standalone task/stream/nested and release/LTO/strip fixtures to verify factory-free inspection and execution.
Keep controlled experiments local under ignored `target/`; run affected-package tests and required hooks before updating the PR.
