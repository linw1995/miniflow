## MODIFIED Requirements

### Requirement: Bind item, key, and index within a scoped body

`mfn-core` SHALL register `builtin.iteration`, and workflows using it MUST explicitly declare a package that registers
the kind. The compiler SHALL bind an engine-prepared body to the registered implementation, and
the node package SHALL own item scheduling, result collection, and failure policies. The node SHALL expose
one required input `items` accepting arrays or JSON objects and one required array output `results`.
Other input shapes MUST fail at execution before the body runs, regardless of the item failure policy.
Its body SHALL contain ordinary nodes,
optional data and control edges, and one required result selection. The reserved `%iteration` source SHALL expose the
current array element or map value as `item`, the map key as `key` (JSON null for arrays), and its zero-based index
as `index`. Map entries SHALL be visited in lexicographic key order, with `index` assigned in that order.
The body source MUST NOT expose the legacy `items` port. Port contracts SHALL follow the selected runtime and
node package versions; the workflow JSON `version` MUST NOT select a compatibility alias.
Each item SHALL use a fresh context; outputs from
another item MUST NOT be visible. The body MUST NOT contain the reserved source ID, another Iteration node, or a structured Loop construct.

#### Scenario: Transform an array

- **WHEN** the input is `[1, 2, 3]` and the body returns `item * 2 + index`
- **THEN** the collected output is `[2, 5, 8]`

#### Scenario: Transform a map

- **WHEN** the input is `{"b": 2, "a": 1}` and the body returns `key + ':' + string(item * 2 + index)`
- **THEN** the collected output is `["a:2", "b:5"]` in both execution modes

#### Scenario: Preserve array key semantics

- **WHEN** the input is `[1, 2]` and the body selects `%iteration.key`
- **THEN** the collected output is `[null, null]`

#### Scenario: Process an empty collection

- **WHEN** the input is an empty array or object
- **THEN** the output is empty and the validated body is not invoked

#### Scenario: Reject a scalar input

- **WHEN** `items` is null, a boolean, a number, or a string
- **THEN** execution fails with an array-or-object input diagnostic before the body is invoked

#### Scenario: Reject an obsolete body port

- **WHEN** a body uses `%iteration.items` in a data edge, control edge, result selection, or context reference
- **THEN** validation fails for the missing port, including for older supported workflow JSON versions
- **AND** upgrading the definition requires replacing those references with `%iteration.item`

#### Scenario: Reject a scope violation

- **WHEN** a body redefines `%iteration`, includes another Iteration node or a structured Loop construct, or selects a missing result port
- **THEN** build validation fails before installation with the outer iteration ID and body error

#### Scenario: Reject an undeclared Iteration package

- **WHEN** a Flow uses `builtin.iteration` without declaring a package that registers it
- **THEN** build validation reports the unavailable kind and preserves an existing installed executable
