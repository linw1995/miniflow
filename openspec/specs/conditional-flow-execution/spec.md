# conditional-flow-execution Specification

## Purpose

Evaluate ordered conditions from prior node outputs in a per-run context and activate one downstream branch independently of incoming data bindings.

## Requirements

### Requirement: Configure at least one condition and at least two outputs

`builtin.if_else` SHALL require a nonempty ordered `branches` array of objects with an `id` and a `condition`. IDs MUST
match `[A-Za-z_][A-Za-z0-9_-]*`, MUST be unique and case-sensitive, and MUST NOT equal `else`. Unknown configuration,
branch, condition, and source fields MUST be rejected. For `N >= 1` configured conditions, the node SHALL expose no data
input ports, exactly `N` outputs named `<id>`, and one mandatory fallback output named `else`. Incoming control edges
SHALL govern activation, while conditions SHALL read explicitly referenced prior outputs from context. Outputs SHALL
have type `Boolean` and SHALL be declared optional because each invocation selects only one. Output count MUST be
derived as `N + 1 >= 2`.

#### Scenario: Configure the minimum conditional node

- **WHEN** configuration contains one branch with ID `accepted` and a valid predicate
- **THEN** the node has one configured condition, no data inputs, and exactly two outputs `accepted` and `else`

#### Scenario: Configure else-if branches

- **WHEN** configuration contains three distinct branch IDs with valid predicates
- **THEN** the node has three configured conditions, no data inputs, and exactly four outputs, including `else`

#### Scenario: Reject invalid configuration

- **WHEN** branches are missing or empty, an ID is malformed, duplicated, or reserved, a condition is missing, or an unknown field attempts to configure output count
- **THEN** build validation fails with the node ID and the configuration error before executable installation

#### Scenario: Preserve bindings when precedence changes

- **WHEN** a user reorders branch entries without changing their IDs
- **THEN** the condition precedence changes while existing output port names and incoming control bindings remain unchanged

### Requirement: Separate incoming activation from condition data access

Workflow definitions SHALL accept an optional `control_edges` array defaulting to empty, whose entries contain
`from_node`, `from_output`, and `to_node`. These edges SHALL establish execution dependencies without creating target
input bindings. Existing data edges SHALL retain their data-binding behavior. The combined dependency graph MUST be
acyclic. A control dependency SHALL be active when its source output is produced, including false or null values;
explicit skip state SHALL make it inactive. Multiple incoming dependencies SHALL all be required for activation.
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

### Requirement: Read prior outputs from an isolated execution context

Each workflow invocation SHALL have a fresh context containing outcomes of previously resolved nodes. A workflow run
SHALL resolve each scheduled node at most once, following its validated execution order. Nodes SHALL receive read-only
access to completed context outputs. Reference declarations SHALL be used for compile-time ordering validation; runtime
context access SHALL NOT require an additional node-registration or per-read authorization protocol. The runtime MUST
publish validated completed or skipped outcomes only after a node resolves, MUST NOT expose partial outputs, and MUST
NOT retain context values between runs. Context reads SHALL distinguish produced values, explicitly skipped outcomes,
and unavailable outputs. Reads before production and unexpectedly omitted outputs MUST both fail. A context reference
alone MUST NOT activate or skip the consuming node.

#### Scenario: Read a transitive predecessor

- **WHEN** an active router references a declared output from an earlier node on its dependency chain
- **THEN** it reads the stored output even when that node is not its immediate incoming source

#### Scenario: Isolate repeated runs

- **WHEN** the same flow runs twice with different prior node outputs
- **THEN** the second run's conditions observe only its own context values

#### Scenario: Reject unavailable context reads

- **WHEN** a node reads an output that has not been produced or explicitly skipped
- **THEN** execution reports an unavailable output error rather than reading stale or partial data

### Requirement: Qualify context outputs with the producing node ID

Context outputs SHALL be addressed by the exact key `${node_id}.${output_name}`, where `node_id` is the workflow
definition ID and `output_name` is the local output name. Runtime publication SHALL add the prefix exactly once;
plugins SHALL continue to declare and return local names. Produced values and output availability states SHALL use
the same qualified identity. IDs MUST be case-sensitive and MUST NOT be normalized, split on dots, or interpreted
as nested field paths. Nested value access SHALL use the separate JSON Pointer path.

The system MUST validate qualified-ID uniqueness across all effective declared outputs before execution, including
ports that might be skipped. Dots in either component SHALL be accepted when the resulting key is unique. Collisions
MUST fail validation with the qualified ID and both source pairs; the runtime MUST NOT resolve ambiguity by execution
order or overwrite a prior entry. Selected workflow results SHALL retain their explicitly configured result aliases.

#### Scenario: Keep identically named outputs from different nodes

- **WHEN** nodes `load_order` and `load_customer` both produce local output `value`
- **THEN** context stores their values separately as `load_order.value` and `load_customer.value`

#### Scenario: Qualify branch activation outputs

- **WHEN** node `route` selects its local `preferred` branch and skips `else`
- **THEN** `route.preferred` resolves to true and `route.else` resolves to an explicit skipped state

#### Scenario: Resolve an exact key containing multiple dots

- **WHEN** node `order.fetch` declares output `value` and no other output produces the same key
- **THEN** reference `order.fetch.value` resolves to that output without guessing how to split the key

#### Scenario: Reject a qualified-ID collision

- **WHEN** node `a.b` declares output `c` and node `a` declares output `b.c`
- **THEN** validation rejects their shared key `a.b.c` and identifies both source pairs, even if one could be skipped

#### Scenario: Preserve a selected result alias

- **WHEN** a workflow selects node `load_order`, port `value`, under result name `order`
- **THEN** the result key is `order` while the context key remains `load_order.value`

### Requirement: Evaluate simple predicates against referenced context outputs

Each condition SHALL contain a `source` object with required qualified `output` ID and JSON Pointer `path` fields, plus
an `operator`. The empty path SHALL select the referenced output root. Different branches SHALL be able to reference
different prior nodes and ports. Nested fields, array indices, and `~0` / `~1` escapes SHALL be supported. Malformed pointer syntax MUST fail
configuration validation. The supported operators SHALL be `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `exists`, and
`not_exists`. Equality operators MUST require a scalar literal `value`; ordering operators MUST require a numeric
literal `value`; existence operators MUST reject a `value` field. Unknown operators and invalid literals MUST fail
configuration validation for every branch, including branches not reached during execution.

#### Scenario: Compare a stored output field directly

- **WHEN** `ctx.outputs["load_order.value"]` contains `{"amount": 1500}` and the reached condition uses `{"source": {"output": "load_order.value", "path": "/amount"}, "operator": "gte", "value": 1000}`
- **THEN** the condition evaluates to true without an upstream comparison node or a data edge binding that value to the router

#### Scenario: Compare an upstream boolean without coercion

- **WHEN** a referenced context output is JSON `true` and the reached condition uses path `""`, operator `eq`, and literal true
- **THEN** the condition evaluates to true using the same field comparison contract

#### Scenario: Resolve nested and escaped paths

- **WHEN** conditions address `/items/0/price`, `/a~1b`, and `/a~0b` within stored outputs containing those values
- **THEN** lookup selects the first item's price, the key `a/b`, and the key `a~b`, respectively

#### Scenario: Reject malformed predicate configuration

- **WHEN** a condition has an unknown operator, malformed pointer escape, missing required literal, container literal, nonnumeric ordering literal, or an existence operator with a literal
- **THEN** build validation fails with node and branch context before executable installation

### Requirement: Preserve typed comparison and field presence semantics

Scalar equality SHALL compare strings exactly and case-sensitively, booleans and null by value, and numbers numerically.
Different scalar types SHALL be unequal, with `ne` returning the negation of equality. No implicit type conversion
SHALL occur. Numeric ordering SHALL require numeric operands. Reached comparisons resolving to an array or object MUST
fail. Numeric comparisons MUST preserve distinct signed and unsigned 64-bit integer values without rounding them
through floating-point conversion; equivalent runtime numeric values such as `1` and `1.0` SHALL compare equal.

`exists` SHALL return whether the path resolves and `not_exists` SHALL return its inverse. Present null values MUST
count as existing. An unresolved path, including traversal through a scalar or an invalid array index, MUST fail a
reached comparison instead of evaluating to false. Data-dependent predicate errors MUST identify the node, branch,
qualified source output ID, path, and operator and MUST terminate the workflow instead of selecting `else`.
An explicitly skipped output or producer SHALL return false for `exists` and true for `not_exists`; reached comparisons
against it MUST fail. Pending producers and unexpectedly omitted output ports MUST fail for every operator, including
existence checks. A null output SHALL count as produced and existing at its root path.

#### Scenario: Keep missing fields distinct from null

- **WHEN** a referenced output contains `{"status": null}`
- **THEN** `exists` at `/status` is true, `eq` with null at `/status` is true, and `not_exists` at `/absent` is true

#### Scenario: Reject a missing comparison field

- **WHEN** a reached `ne` condition references an absent field
- **THEN** execution fails with a missing-field diagnostic instead of matching that branch

#### Scenario: Preserve typed equality

- **WHEN** a reached `eq` condition compares the string `"100"` with the number `100`
- **THEN** it returns false without converting the string to a number

#### Scenario: Reject incompatible ordering

- **WHEN** a reached numeric ordering condition resolves to a string, boolean, null, object, or array
- **THEN** execution fails with condition context instead of coercing the value or selecting the fallback

#### Scenario: Preserve integer comparison precision

- **WHEN** a predicate compares runtime numbers `9007199254740993` and `9007199254740992`
- **THEN** equality is false and the first value orders greater than the second

#### Scenario: Compare equivalent numeric forms

- **WHEN** a predicate compares runtime numeric values `1` and `1.0`
- **THEN** equality is true and strict greater-than and less-than are false

### Requirement: Select the first matching branch with short-circuit evaluation

Each executed conditional node SHALL evaluate configured predicates against its current context in array order and MUST stop
evaluating after the first true predicate. If every predicate evaluates to false, it SHALL select `else`. It SHALL
produce JSON true on exactly one output as an activation value and explicitly skip every other output. It MUST NOT
forward an incoming payload or copy context into branch outputs. Predicate syntax and static references MUST be validated
for all branches, while data-dependent errors SHALL apply only to reached predicates.

#### Scenario: Select the first branch

- **WHEN** the first condition is true
- **THEN** its output produces true and all other branch outputs, including `else`, are explicitly skipped

#### Scenario: Select an else-if branch

- **WHEN** the first condition is false and the second condition is true
- **THEN** the second output produces true and every other output is explicitly skipped

#### Scenario: Resolve multiple true conditions

- **WHEN** several conditions are true
- **THEN** only the earliest configured matching branch produces true

#### Scenario: Select the fallback

- **WHEN** every condition is false
- **THEN** `else` produces true and all conditional outputs are explicitly skipped

#### Scenario: Skip evaluation of a later predicate

- **WHEN** an earlier predicate matches and a later statically valid predicate references a skipped output or missing or incompatible business field
- **THEN** the earlier branch activates and the later predicate is not evaluated

#### Scenario: Reject malformed syntax in a later branch

- **WHEN** an earlier predicate could match every input but a later predicate has an unknown operator
- **THEN** configuration validation fails before workflow execution

#### Scenario: Leave business data in its producer's context entry

- **WHEN** a predicate reads `load_order.value` containing `{"amount": 1500, "id": "order-1"}` and matches
- **THEN** that context value remains unchanged and the selected router output is true rather than a copy of the order

#### Scenario: Compare a null context value

- **WHEN** a referenced output is JSON null and the first predicate compares its root with a literal null using `eq`
- **THEN** the branch activates and the context null remains distinguishable from a skipped or missing output

#### Scenario: Inspect explicit absence without changing activation

- **WHEN** a router is activated by a produced control output and its first predicate uses `not_exists` on a different explicitly skipped output from a prior node
- **THEN** that predicate matches without the reference itself causing the router to be skipped

### Requirement: Propagate explicit skips without executing downstream nodes

Execution SHALL distinguish produced values, explicitly skipped ports or nodes, and unexpectedly absent outputs. After
resolving all data bindings and incoming control edges, any unexpected absence MUST cause an error with source and target context. Otherwise,
any skipped dependency MUST skip the target node without invoking its execution method and MUST propagate skip
state through its outputs. This rule SHALL include connected optional inputs. Unconnected optional inputs MUST NOT
trigger skipping. Independent nodes SHALL remain eligible to execute in the existing deterministic order.

#### Scenario: Suppress unselected business side effects

- **WHEN** an unselected branch feeds a node that would record a side effect or fail if executed
- **THEN** that node's execution method is not called and its downstream outputs are skipped

#### Scenario: Propagate through nested branches and fan-out

- **WHEN** a skipped output feeds several nodes, including another conditional node
- **THEN** all of those dependent nodes and their dependent descendants are skipped unless execution has already failed

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

- **WHEN** a node has data or control dependencies on two mutually exclusive branch outputs
- **THEN** that node is skipped because one connected input is skipped

### Requirement: Preserve plugin errors and ordinary plugin compatibility

Plugins using the documented static-port construction and ordinary execution interfaces SHALL retain their execution
behavior when rebuilt against the updated runtime, provided their produced outputs conform to their declared ports.
This guarantee covers constructor-based registrations and ordinary node execution implementations; changed descriptor
structs and low-level execution helpers require the documented Rust API migration. Third-party plugins SHALL be able to
describe explicit skipped output ports without reserved kind names. Produced output names MUST belong to the node's
effective declared outputs. Explicit skip markers MUST reference declared non-required outputs and MUST NOT overlap
produced values. An absent output without a skip marker MUST remain a missing-output error when referenced. Node errors
MUST remain failures and MUST NOT be converted to skip states.

#### Scenario: Run an unchanged ordinary plugin

- **WHEN** a plugin retains its constructor-based static registration and ordinary execution implementation and produces declared outputs
- **THEN** it can run in a conditional workflow and its outputs are treated as produced values

#### Scenario: Reject an invalid skip declaration

- **WHEN** a plugin explicitly skips an unknown or required output, or both produces and skips the same output
- **THEN** execution fails with the node and output context

#### Scenario: Reject an undeclared produced output

- **WHEN** a plugin returns a value under an output name absent from its effective port declaration
- **THEN** execution fails with the node and output context before publishing that node's result

#### Scenario: Preserve an optional port omission error

- **WHEN** a plugin omits a referenced optional output without explicitly skipping it
- **THEN** execution reports a missing output

#### Scenario: Preserve a selected branch failure

- **WHEN** a selected branch's downstream node returns an execution error
- **THEN** the workflow fails with that node's diagnostic

### Requirement: Select optional workflow results explicitly

Workflow output selections SHALL accept `optional`, a boolean defaulting to false. A produced value MUST be included
under the selected name, including JSON `null`. An explicitly skipped source MUST fail a required selection and MUST
omit the key for an optional selection. An unexpectedly absent output MUST fail either selection. Optional selections
MUST still reference valid node ports and unique output names. Existing schema `2026-09-26` definitions without this
field SHALL retain their output semantics; default-false values SHALL be omitted when serialized.

#### Scenario: Omit an unselected optional result

- **WHEN** a selected output has `optional: true` and its source port or node is skipped
- **THEN** the result JSON omits that selected name

#### Scenario: Reject an unselected required result

- **WHEN** a selected output has no optional flag or has `optional: false` and its source is skipped
- **THEN** execution fails with the selected name, source node, and port

#### Scenario: Preserve null in an optional result

- **WHEN** an optional selected output produces JSON `null`
- **THEN** the result includes that key with value `null`

#### Scenario: Preserve unexpected output failure

- **WHEN** an optional selection references a declared output omitted without a skip marker
- **THEN** execution fails with a missing-output diagnostic

#### Scenario: Produce an empty result object

- **WHEN** all selected outputs are optional and their sources are explicitly skipped
- **THEN** execution succeeds with an empty JSON object

#### Scenario: Validate optional selections normally

- **WHEN** an optional selection references an unknown port or repeats another selected name
- **THEN** validation fails before executable installation
