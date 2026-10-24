## ADDED Requirements

### Requirement: Accept display-only node clusters

Definition version `2026-10-23` SHALL accept an optional `clusters` array in every graph scope. Clusters annotate
existing nodes and MUST NOT add nodes, ports, bindings, dependencies, or scheduling boundaries. Older versions MUST
reject cluster declarations.

#### Scenario: Compile a clustered definition

- **WHEN** a `2026-10-23` definition declares clusters over existing nodes
- **THEN** compilation produces the same execution plan and results as the same definition without clusters

#### Scenario: Reject clusters in an older version

- **WHEN** a definition using version `2026-10-19` or earlier contains a `clusters` field
- **THEN** parsing fails with a diagnostic requiring version `2026-10-23`

### Requirement: Validate node cluster metadata

Each cluster SHALL require a bounded nonblank single-line `label`, a bounded nonblank `annotation`, and a nonempty,
unique list of member IDs from the containing scope. A node MAY belong to only one cluster, and reserved synthetic
sources MUST NOT be members.

#### Scenario: Reject ambiguous membership

- **WHEN** a cluster names an unknown node, a reserved synthetic source, or a node already owned by another cluster
- **THEN** validation fails before installation and identifies the invalid cluster membership
