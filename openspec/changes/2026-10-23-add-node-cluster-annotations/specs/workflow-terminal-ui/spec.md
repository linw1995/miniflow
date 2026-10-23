## ADDED Requirements

### Requirement: Render annotated node clusters

The terminal UI SHALL render each declared cluster with its short label. Selecting a member or folded cluster SHALL
show the long annotation in Details. Fold state SHALL be retained per scope and connection view for the session and
MUST NOT change execution, observation completeness, or business results.

#### Scenario: Inspect cluster annotations

- **WHEN** the operator selects a node in a declared cluster
- **THEN** Details shows the cluster label, annotation, and member count without exposing business values

#### Scenario: Preserve navigation and bodies

- **WHEN** clusters are folded while inspecting a Loop or Iteration body
- **THEN** fold state is scoped to that body and existing body navigation keys continue to work

### Requirement: Fold annotated node clusters

Pressing `c` on a cluster member SHALL collapse the cluster into one label-bearing display vertex with an aggregate
status. Pressing `c` on a collapsed cluster SHALL expand it. Collapsed vertices SHALL remap external data, control, and
success connections, hide internal connections, and retain member identity for selection and Details.

#### Scenario: Fold a cluster

- **WHEN** the operator selects a node in an expanded cluster and presses `c`
- **THEN** the cluster becomes one vertex with its label and aggregate status, and external connections attach to it

#### Scenario: Expand a cluster

- **WHEN** the operator presses `c` on a collapsed cluster
- **THEN** its member nodes and annotation return and the previously focused member is selected again

#### Scenario: Inspect member state while folded

- **WHEN** a collapsed cluster contains running, failed, or incomplete members
- **THEN** its aggregate status reflects the most significant member state and Details lists the member outcomes
