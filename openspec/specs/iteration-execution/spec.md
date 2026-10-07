# iteration-execution Specification

## Purpose

Run a validated body graph once per array element or map entry and collect its selected results.

## Requirements

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

### Requirement: Bound scheduling and preserve result order

`mode` SHALL default to `sequential`. In sequential mode, the body SHALL run one item at a time using the same scoped body execution mechanism as Loop. In `parallel` mode, at most ten items SHALL run concurrently. Successful results SHALL retain array input order or lexicographic map key order regardless of completion order.

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
These policies SHALL apply equally to array elements and map entries, using the map's sorted entry positions.

#### Scenario: Continue with null

- **WHEN** the middle item fails under `continue_on_error`
- **THEN** output contains the first result, null, and the third result

#### Scenario: Remove failed results

- **WHEN** the middle item fails under `remove_failed`
- **THEN** output contains only the first and third results

#### Scenario: Terminate without partial publication

- **WHEN** any item fails under `terminate`
- **THEN** the Iteration node fails and does not publish `results`

### Requirement: Share struct-defined Iteration inputs across task interfaces

Iteration SHALL expose a typed input struct containing one required shared JSON value named items and use its declaration during preparation. Its existing dynamic task interface SHALL remain supported through runtime-owned decoding. Missing or unknown bindings SHALL retain typed runtime decode errors. Supplied scalar items SHALL retain the array-or-object domain error. Body-dependent output declarations and iteration policies SHALL remain unchanged.

#### Scenario: Prepare a typed iteration

- **WHEN** the provider is prepared with a typed body result or continue-on-error policy
- **THEN** its items input is derived as required Any and its results output retains the body-dependent list descriptor

#### Scenario: Preserve dynamic callers

- **WHEN** a caller invokes Iteration through the existing dynamic task interface with valid array or map inputs
- **THEN** runtime conversion supplies the typed struct and iteration preserves ordering and shared child payloads

#### Scenario: Invoke the typed interface

- **WHEN** a caller supplies the typed Iteration input struct directly
- **THEN** its body executes with the same item scopes and result behavior as dynamic invocation

#### Scenario: Reject invalid bindings before the body

- **WHEN** direct dynamic invocation omits items or supplies an undeclared binding
- **THEN** runtime decoding rejects the binding with a typed source and does not invoke the body

#### Scenario: Keep scalar rejection in the provider

- **WHEN** either task interface receives a supplied scalar items value
- **THEN** Iteration reports its array-or-object diagnostic before executing any item
