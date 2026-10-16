## ADDED Requirements

### Requirement: Support published configuration in the runtime manifest codec

The runtime manifest codec SHALL support version `2026-10-15` with root-ID-keyed `node_configurations`, separate
from the graph and interface. It SHALL accept legacy manifests without the field, omit empty maps on write,
and reject nonempty configuration in `2026-10-07` or entries for unknown roots on both read and write.
Existing framing, bounds, duplicate-key rejection, and graph/interface validation SHALL remain in force.

#### Scenario: Round-trip published configuration

- **WHEN** a current-version manifest contains configuration for a declared root node
- **THEN** framing and decoding preserve the configuration together with the graph and interface

#### Scenario: Preserve legacy manifests

- **WHEN** a supported older manifest has no published configuration
- **THEN** it remains readable and serializes without an added configuration field

#### Scenario: Reject incompatible or orphaned configuration

- **WHEN** configuration names an unknown root node or nonempty configuration uses the older payload version
- **THEN** both manifest encoding and decoding reject it
