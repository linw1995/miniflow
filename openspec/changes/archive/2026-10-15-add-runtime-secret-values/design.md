## Design

Keep secret metadata on ValueRef alongside its shared immutable payload. Marking a
container marks its descendants so borrowed field/index/pointer access and typed
shared codecs preserve sensitivity. Existing aliases remain unchanged. Equality,
type validation, accessors, and Serde retain business JSON semantics.

Store only the marker for the value itself. Inspect descendants on demand when
checking a public container for secrets; do not cache derived sensitivity or
serialized/redacted values without representative workload evidence. Derived values
explicitly inherit sensitivity from their source values through inherit_secrets.
Owned Rust decoding and JSON round trips do not carry metadata.

Project snapshots to ordinary JSON values with a fixed [REDACTED] string before
callbacks or direct recording. Both public entry points need protection; recording
must project before equality checks, pointer alias lookup, interning, and retention.
Public subtrees retain shared payloads. The existing version 1 dictionary and
receiver remain sufficient. Display and Debug use the same projection.

Merge scope values by shared identity including the root marker, rather than JSON
equality, so equal payloads with changed metadata are published. Keep error types,
module boundaries, and source chains unchanged.

## Boundaries

Templates are deferred to #145. Nodes must explicitly mark new values derived from
secrets. Raw strings copied into custom errors, logs, span attributes, or object
keys have no sensitivity metadata. Business serialization and final runner output
retain original values. This feature does not promise memory zeroization or
retroactive removal of values observed before they were marked.

## Validation

Use pinned Nix tools, targeted runtime/compiler tests, and prek -a. Ablate the
redaction entry points, recursive marking, metadata merge, and identity comparison.
Use on-demand descendant inspection and avoid speculative caching. Keep experiment scripts,
logs, and measurements only under ignored target/. Review implementation and
contract changes before archiving.
