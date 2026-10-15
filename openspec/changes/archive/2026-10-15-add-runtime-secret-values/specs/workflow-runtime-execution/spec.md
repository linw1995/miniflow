## ADDED Requirements

### Requirement: Preserve secret runtime value metadata

Runtime values SHALL support an immutable secret marker without changing their
JSON type, business accessors, equality, validation, or serialization. Marking a
container SHALL mark all descendants. Cloning, routing, shared typed codecs, and
field/index/pointer extraction MUST retain markers. Existing unmarked aliases
SHALL remain unchanged.

#### Scenario: Extract a field from a secret object

- **WHEN** a caller marks an object secret and extracts a nested field
- **THEN** both the object and extracted value remain secret and business reads return their original data
- **AND** aliases retained before marking remain unmarked

#### Scenario: Route sensitive values through tasks and streams

- **WHEN** marked values pass through forwarding tasks or accumulate in a Batch output
- **THEN** runtime results preserve their data and markers, including secret elements inside public containers

### Requirement: Propagate sensitivity into derived values explicitly

The runtime SHALL expose a way to mark a computed value when any supplied source
contains a secret. Public containers SHALL report sensitivity in their descendants.
The runtime MUST NOT infer that every output of a node is secret merely because
one input is secret. Ordinary owned decoding and JSON round trips SHALL NOT claim
to retain sensitivity metadata.

#### Scenario: Compute a credential header

- **WHEN** a caller computes a new string and inherits sensitivity from a secret token or a container holding it
- **THEN** the complete computed string becomes secret while its business value remains available

### Requirement: Retain metadata-only scope updates

Execution-domain scope merging MUST retain changes to secret metadata even when
the business JSON values compare equal.

#### Scenario: Mark an unchanged Loop variable secret

- **WHEN** a domain publishes a Loop variable with the same payload and a new secret marker
- **THEN** the enclosing scope and its published source output retain the marker
