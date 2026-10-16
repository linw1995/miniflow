## Decisions

`NodeMetadata::public_config` contains only author-selected inspection values. `Flow::node_configurations`,
`StreamPlan::node_configurations`, and compiler callers share `published_node_configurations`; the map alias keeps
these signatures consistent. Collection omits empty entries and preserves exact root IDs.

The manifest codec supports `2026-10-15` with optional `node_configurations`. It retains the existing framing,
size bounds, duplicate-key rejection, typed error sources, and graph/interface validation. Nonempty configuration
requires the new version, and every entry must identify a root node. Existing compiler output stays at `2026-10-07`.

Inspection fields are skipped by ordinary `NodeMetadata` serialization. That serialization is also used by
`verify_generated_metadata`, so including inspection fields would impose an unintended execution drift check.
Manifest serialization remains explicit and independent. Providers remain responsible for excluding private data.

`ConfigTemplate::evaluate` and `decode_unique_json` reuse existing implementations. Host callers can evaluate
already-bound values without constructing execution state, and decode bounded documents with unique object keys.
Callers retain responsibility for input byte limits. No new facade, decoder, or visibility-only file split is needed.

## Review boundaries

The extracted public APIs have concrete consumers in the source compiler and chat host. Their absence from this
standalone slice is not evidence that they are unused. No model-specific runtime errors, parent-owner protocol,
configuration drift validator, dependency changes, or execution scheduling changes are included.

## Validation

Run controlled simplifications and negative controls, retaining experiment details only under ignored `target/`.
Run the affected-package selector and pinned hooks. Review the final implementation and spec deltas before archival.
