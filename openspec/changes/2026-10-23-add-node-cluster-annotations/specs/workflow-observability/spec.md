## ADDED Requirements

### Requirement: Describe display-only node clusters

Runner descriptions for version `2026-10-23` SHALL preserve cluster labels, annotations, and scope-local member node
IDs for the root graph and every described body. Description validation SHALL enforce the same membership, text,
synthetic-source, and bound rules as definitions. Descriptions MUST NOT expose cluster metadata through lifecycle
events, snapshots, or business values. Readers SHALL reject unsupported description versions before execution rather
than silently ignoring clusters.

#### Scenario: Inspect a clustered runner

- **WHEN** a host reads the manifest of a `2026-10-23` runner that declares clusters
- **THEN** its description contains the cluster labels, annotations, and members without executing nodes

#### Scenario: Preserve body clusters

- **WHEN** a Loop or Iteration body declares clusters
- **THEN** the corresponding body description carries those clusters with scope-local member IDs

#### Scenario: Reject unsupported description versions

- **WHEN** an older reader encounters a description carrying clusters
- **THEN** it rejects the unsupported version before starting execution
