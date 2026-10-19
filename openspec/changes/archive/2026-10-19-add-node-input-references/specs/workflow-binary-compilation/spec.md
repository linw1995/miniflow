## MODIFIED Requirements

### Requirement: Describe a compiled workflow without executing nodes

Generated runners SHALL embed a date-versioned graph description of workflow identity, nodes, data bindings, output
gates, node-success edges when present, and deterministic order. Hosts SHALL read it without build inputs or plugin
execution. Success edges SHALL identify nodes without fabricated ports and require a supported description version.
Descriptions MUST exclude config/business values and MUST NOT infer unconnected dynamic ports from edges.

#### Scenario: Inspect a standalone binary

- **WHEN** a generated runner's manifest is read after its build inputs are removed
- **THEN** it provides its supported graph description without executing any node operation

#### Scenario: Describe a graph with configuration-dependent ports

- **WHEN** a compiled workflow contains two instances of a conditional node with different branch names
- **THEN** its description contains the configured data/control relationships without claiming to enumerate unconnected dynamic ports or including predicate configuration

#### Scenario: Avoid construction diagnostics

- **WHEN** a linked plugin factory normally prints diagnostics during validation
- **THEN** manifest inspection does not invoke that factory or start a process

#### Scenario: Describe a zero-output success prerequisite

- **WHEN** a compiled workflow orders two Tasks without passing a value
- **THEN** its supported description includes the node-success dependency without inventing an output port
