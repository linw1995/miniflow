# iteration-execution Specification

## Purpose

Run a validated body graph once per array element and collect its selected results.

## Requirements

### Requirement: Bind item and index within a scoped body

`mfn-core` SHALL register `builtin.iteration`, and workflows using it MUST explicitly declare a package that registers
the kind. The compiler SHALL bind an engine-prepared body to the registered implementation, and
the node package SHALL own item scheduling, result collection, and failure policies. The node SHALL expose
one required array input `items` and one required array output `results`. Its body SHALL contain ordinary nodes,
optional data and control edges, and one required result selection. The reserved `%iteration` source SHALL expose the
current element as `items` and its zero-based index as `index`. Each item SHALL use a fresh context; outputs from
another item MUST NOT be visible. The body MUST NOT contain the reserved source ID, another Iteration node, or a structured Loop construct.

#### Scenario: Transform an array

- **WHEN** the input is `[1, 2, 3]` and the body returns `items * 2 + index`
- **THEN** the collected output is `[2, 5, 8]`

#### Scenario: Process an empty array

- **WHEN** the input is empty
- **THEN** the output is empty and the validated body is not invoked

#### Scenario: Reject a scope violation

- **WHEN** a body redefines `%iteration`, includes another Iteration node or a structured Loop construct, or selects a missing result port
- **THEN** runner validation fails before installation with the outer iteration ID and body error

#### Scenario: Reject an undeclared Iteration package

- **WHEN** a Flow uses `builtin.iteration` without declaring a package that registers it
- **THEN** runner validation reports the unavailable kind and preserves an existing installed executable

### Requirement: Bound scheduling and preserve result order

`mode` SHALL default to `sequential`. In sequential mode, the body SHALL run one item at a time using the same scoped body execution mechanism as Loop. In `parallel` mode, at most ten items SHALL run concurrently. Successful results SHALL retain input order regardless of completion order.

#### Scenario: Complete out of order

- **WHEN** independent parallel items finish in an order different from their input positions
- **THEN** `results` retains the original input positions

### Requirement: Apply item failure policies

`on_error` SHALL default to `terminate`. `terminate` SHALL stop sequential execution at the first
failure or stop scheduling parallel work after a failure, drain started work, report the lowest
failed started index, and publish no partial result. `continue_on_error` SHALL place JSON null at
each failed position. `remove_failed` SHALL omit failed positions and retain successful order. A
required body result that is skipped SHALL count as a failed item. Errors SHALL include the outer
node and failing item index.

#### Scenario: Continue with null

- **WHEN** the middle item fails under `continue_on_error`
- **THEN** output contains the first result, null, and the third result

#### Scenario: Remove failed results

- **WHEN** the middle item fails under `remove_failed`
- **THEN** output contains only the first and third results

#### Scenario: Terminate without partial publication

- **WHEN** any item fails under `terminate`
- **THEN** the Iteration node fails and does not publish `results`
