## MODIFIED Requirements

### Requirement: Separate incoming activation from condition data access

Workflow definitions SHALL accept an optional `control_edges` array defaulting to empty, whose entries contain
`from_node`, `from_output`, and `to_node`. These edges SHALL establish execution dependencies without creating target
input bindings. Existing data edges SHALL retain their data-binding behavior. The combined dependency graph MUST be
acyclic. A control dependency SHALL be active when its source output is produced, including false or null values;
explicit skip state SHALL make it inactive. Ordinary data inputs, resolved exclusive input groups, and incoming control dependencies SHALL all be required for activation. Candidate skips within a successfully resolved exclusive group MUST NOT independently deactivate the target. Every candidate SHALL still establish a scheduling dependency.
Condition source references MUST NOT implicitly create dependencies or supply input values.

#### Scenario: Trigger from one node and inspect another

- **WHEN** `load_order` precedes `audit`, `audit.done` activates `route`, and a predicate references `load_order.value`
- **THEN** `route` evaluates the stored order output without using `audit.done` as its condition operand

#### Scenario: Gate a node independently of its data input

- **WHEN** a downstream node binds business data from `load_order.value` and has an incoming control edge from a skipped router output
- **THEN** the downstream node is skipped despite its business data being available

#### Scenario: Activate using a produced false or null value

- **WHEN** a control source produces JSON false or null and every other dependency is available
- **THEN** the target becomes active because control edges check output availability rather than truthiness

#### Scenario: Preserve existing data-only definitions

- **WHEN** a definition omits `control_edges`
- **THEN** its existing data bindings and dependency order retain their previous semantics

#### Scenario: Keep a separate gate mandatory after selection

- **WHEN** an exclusive input group resolves one produced value but a separate ordinary input or control dependency is skipped
- **THEN** the consumer remains skipped without invoking its execution method

### Requirement: Propagate explicit skips without executing downstream nodes

Execution SHALL distinguish produced values, explicitly skipped ports or nodes, and unexpectedly absent outputs. After
resolving all data bindings, including exclusive input groups, and incoming control edges, any unexpected absence or exclusive-group ambiguity MUST cause an error with source and target context. Otherwise,
any skipped ordinary input, resolved exclusive input group, or control dependency MUST skip the target node without invoking its execution method and MUST propagate skip
state through its outputs. This rule SHALL include connected optional inputs. Unconnected optional inputs MUST NOT
trigger skipping. Independent nodes SHALL remain eligible to execute in the existing deterministic order.

In streaming execution, ordinary skip propagation SHALL remain within the current message domain. A skipped input to a collecting node SHALL contribute no element and create no output-domain frame or downstream skip. It MUST NOT alter other buffered elements or their deadline. Unexpected absence still takes precedence over a skip at that input.

#### Scenario: Suppress unselected business side effects

- **WHEN** an unselected branch feeds a node that would record a side effect or fail if executed
- **THEN** that node's execution method is not called and its downstream outputs are skipped

#### Scenario: Propagate through nested branches and fan-out

- **WHEN** a skipped output feeds several nodes, including another conditional node
- **THEN** ordinary dependent nodes and their descendants propagate skips unless execution has already failed; an explicit exclusive input group with one produced alternative resolves successfully and stops that candidate skip from propagating through the group

#### Scenario: Execute an independent node

- **WHEN** a node has no dependency on an unselected branch and its inputs are available
- **THEN** it executes normally in the deterministic topological order

#### Scenario: Skip a node with a skipped optional input

- **WHEN** one connected optional input is skipped and all other connected inputs are available
- **THEN** the node is skipped

#### Scenario: Leave an unconnected optional input absent

- **WHEN** an optional input has no edge and all connected inputs are available
- **THEN** the node executes with that optional input absent

#### Scenario: Reject missing data even when another input is skipped

- **WHEN** one input references an unexpectedly absent output and another references a skipped output
- **THEN** execution reports the missing output regardless of edge declaration order

#### Scenario: Keep ordinary joins conjunctive

- **WHEN** a node has ordinary data or control dependencies on two mutually exclusive branch outputs, without an exclusive input group
- **THEN** that node is skipped because one connected input is skipped

#### Scenario: Skip one input before a collection boundary

- **WHEN** a conditional branch skips one input while a collector holds earlier accepted values
- **THEN** the skipped input is excluded, the existing batch remains eligible to emit, and no batch-domain skip is invented

#### Scenario: Resume after an explicit exclusive group

- **WHEN** a group has one produced candidate and one explicitly skipped candidate, with all other consumer dependencies available
- **THEN** the consumer runs once and the unselected candidate is not reported as a cause for skipping the consumer
