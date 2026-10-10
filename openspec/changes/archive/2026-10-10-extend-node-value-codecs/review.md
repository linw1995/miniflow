# Review

No blocking findings remain in the reviewed implementation.

## AGENTS.md compliance

The public surface is selected through the existing runtime entry point. Runtime helpers are exposed for downstream
macro expansion; context selectors remain inside their owning modules. No facade modules, visibility-only relocation,
new scoped visibility, dependency updates, or node-specific runtime errors are introduced.

Runtime conversion errors use Snafu selectors, context, and checked invariants. Typed mismatch and directional sources
retain their chains and JSON Pointer paths. The Code node preserves its established vocabulary and existing domain
configuration error boundary rather than adding numeric adapters that stringify runtime mismatch sources.

All source, comments, specification artifacts, and commit text use English. Experimental reports, backup variants, and
coverage files stay under ignored target/. Publication and remote Git operations are outside this task.

## Findings resolved

- Remove the unrelated CEL numeric feature expansion; new runtime descriptors now fail Code configuration gracefully.
- Replace duplicate enum membership checks and unreachable branches with one pattern dispatch and fallible typed errors.
- Generate each port once and remove unused cloning and duplicate test proof fixtures.
- Keep canonical nullable validation in both encoding paths so Value(null) cannot change representation during a transfer.
- Keep defaulted-field metadata and fallback. Optional factory values must survive generated execution without becoming None.

## Test review

Retained tests cover actual codec results, omission/null/value states, range and finite constraints, shared identity and
lazy decoder invocation, enum diagnostics, nesting bounds, typed source paths, and generated/dynamic output parity.
Generated coverage requires an eligible new-codec segment but does not prescribe default fallback representation.
Optional default factories make the fallback regression observe incorrect business output rather than a compiler error.

Reversible production and test probes support the retained boundary guards and adopted reductions. Detailed records are
local under target/ablation/node-value-codecs/. Final verification is recorded separately before commit and archival.
