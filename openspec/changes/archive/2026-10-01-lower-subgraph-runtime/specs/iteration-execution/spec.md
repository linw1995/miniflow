## MODIFIED Requirements

### Requirement: Bind item and index within a scoped body

`mfn-core` SHALL register `builtin.iteration`, and workflows using it MUST explicitly declare a package that registers
the kind. The compiler SHALL bind an engine-prepared body to the registered implementation, and
the node package SHALL own item scheduling, result collection, and failure policies. The node SHALL expose
one required array input `items` and one required array output `results`. Its body SHALL contain ordinary nodes,
optional data and control edges, and one required result selection. The reserved `@iteration` source SHALL expose the
current element as `items` and its zero-based index as `index`. Each item SHALL use a fresh context; outputs from
another item MUST NOT be visible. The body MUST NOT contain the reserved source ID, another Iteration node, or a structured Loop construct.

#### Scenario: Transform an array

- **WHEN** the input is `[1, 2, 3]` and the body returns `items * 2 + index`
- **THEN** the collected output is `[2, 5, 8]`

#### Scenario: Process an empty array

- **WHEN** the input is empty
- **THEN** the output is empty and the validated body is not invoked

#### Scenario: Reject a scope violation

- **WHEN** a body redefines `@iteration`, includes another Iteration node or a structured Loop construct, or selects a missing result port
- **THEN** runner validation fails before installation with the outer iteration ID and body error

#### Scenario: Reject an undeclared Iteration package

- **WHEN** a Flow uses `builtin.iteration` without declaring a package that registers it
- **THEN** runner validation reports the unavailable kind and preserves an existing installed executable

### Requirement: Bound scheduling and preserve result order

`mode` SHALL default to `sequential`. In sequential mode, the body SHALL run one item at a time using the same Loop driver as the Loop node. In `parallel` mode, at most ten items SHALL run concurrently. Successful results SHALL retain input order regardless of completion order.

#### Scenario: Complete out of order

- **WHEN** independent parallel items finish in an order different from their input positions
- **THEN** `results` retains the original input positions
