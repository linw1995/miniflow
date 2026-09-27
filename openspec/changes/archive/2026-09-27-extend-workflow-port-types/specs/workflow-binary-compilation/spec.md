# Spec Delta

## MODIFIED Requirements

### Requirement: Validate workflow structure before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs, existing edge endpoints, selected output
node references and names, and acyclicity. Before installing the executable, its validation mode SHALL validate
registered kinds, configuration, existing ports, required input connections, and at most one connection per input.
Validation mode MUST NOT call node execution methods. The normal mode SHALL execute statically generated orchestration.
Compiler validation SHALL accept statically safe assignments from refined types to the same type, compatible refined collection types, legacy broad supertypes, or `Any`. It SHALL also accept broad or `Any` outputs feeding a refined input when the shared runtime validates the actual value before invoking the target. Disjoint concrete types and incompatible collection shapes MUST fail validation; no implicit coercion SHALL occur.
Validation failures MUST include diagnostics that identify the relevant definition node, port, output, or edge, and MUST NOT produce a successful binary.
The system MUST produce a deterministic topological execution order, using ascending definition ID to break ties between ready nodes.
Port validation SHALL use a node instance's complete configuration-dependent port description when supplied, and otherwise its static registration. Port names MUST be nonempty and unique within each direction. Descriptions MUST depend only on configuration and remain stable between validation and execution. Every node and branch MUST be validated even when it will be skipped during execution.
Graph structure and topological order SHALL include both existing data edges and explicit control edges. Control edges
MUST reference an existing source output and target node, MUST NOT create target input bindings, and MUST NOT contain
duplicate identical entries. Required data-input and type compatibility rules SHALL continue to apply to data edges.
Plugin validation SHALL build a unique index of `${node_id}.${output_name}` for all effective source outputs and check
all declared context references by exact qualified-ID lookup. Qualified-ID collisions MUST fail with both source pairs
before execution, regardless of whether the conflicting outputs could be skipped. Each referenced producer
MUST be a strict ancestor of the consumer through explicit data or control dependencies. Context references MUST NOT
implicitly add dependencies. Unordered, self, and descendant references MUST fail even when a topological tie-break
would place the referenced node first.

#### Scenario: Reject a cyclic workflow

- **WHEN** dependency edges in a definition form a cycle
- **THEN** compilation fails and reports a closed path containing only nodes in the cycle

#### Scenario: Stable order for independent nodes

- **WHEN** multiple nodes are ready to execute at the same point in a DAG
- **THEN** the compiler orders them by ascending definition ID regardless of node or edge declaration order

#### Scenario: Reject an invalid port connection

- **WHEN** an edge references a missing output or input port, or the connected port types are incompatible
- **THEN** compilation fails and reports both endpoints and the validation reason

#### Scenario: Validate a runtime-checked connection

- **WHEN** a source port of type `Any` feeds a target port of type `Int64`
- **THEN** compilation accepts the edge and the runner checks the actual JSON value before invoking the target

#### Scenario: Reject an incompatible refined connection

- **WHEN** a `List(String)` output feeds a `List(Int64)` input
- **THEN** compilation rejects the edge with both endpoints and their types

#### Scenario: Reject an incomplete or ambiguous input

- **WHEN** a required input has no connection or an input has more than one connection
- **THEN** compilation fails and identifies the node and input port

#### Scenario: Reject an invalid selected output

- **WHEN** a selected workflow output references an unknown node or output port, or repeats an output name
- **THEN** compilation fails and identifies the selected output

#### Scenario: Validate separate instances of one dynamic kind

- **WHEN** two instances of `builtin.if_else` declare different branch IDs and branch counts
- **THEN** each instance's edges and selected outputs are checked against its own derived ports

#### Scenario: Reject malformed instance port descriptions

- **WHEN** a plugin supplies an empty port name or duplicate input or output names
- **THEN** validation fails with the node and invalid descriptor context

#### Scenario: Activate a router without a data input

- **WHEN** a conditional node has valid incoming control dependencies and valid ancestor context references
- **THEN** validation succeeds without requiring a payload or condition input port

#### Scenario: Reject an unordered context reference

- **WHEN** a predicate references an existing output from a node outside the consumer's explicit ancestor chain
- **THEN** validation identifies the consumer, branch, and source and requires an explicit dependency instead of adding one

#### Scenario: Reject self or future references

- **WHEN** a predicate references its own node or a downstream node
- **THEN** validation fails before executable installation even if that predicate follows an always-matching branch

#### Scenario: Reject an unknown referenced output

- **WHEN** a predicate references a qualified output ID absent from the effective output index
- **THEN** validation reports the reference and its consumer branch before execution

#### Scenario: Reject ambiguous output identities

- **WHEN** effective output pairs `(a.b, c)` and `(a, b.c)` both form `a.b.c`
- **THEN** validation reports the collision before executable installation without splitting or resolving the key heuristically

#### Scenario: Include control dependencies in structural planning

- **WHEN** data and control edges together form a cycle, or a control edge names an unknown endpoint or duplicates an existing control edge
- **THEN** structural validation fails before runner generation

#### Scenario: Allow transitive source references

- **WHEN** `load_order` precedes `audit`, `audit` precedes `route`, and a route predicate reads `load_order.value`
- **THEN** validation accepts the reference without requiring another direct edge from `load_order` to `route`

#### Scenario: Validate every configured predicate

- **WHEN** a later branch has malformed predicate syntax, an invalid path escape, or an invalid literal for its operator
- **THEN** validation fails with node and branch context even if an earlier branch could match every input

#### Scenario: Validate an unselected branch

- **WHEN** a downstream node on an unselected branch has invalid configuration or a nonexistent port binding
- **THEN** build validation fails without invoking any node execution method
