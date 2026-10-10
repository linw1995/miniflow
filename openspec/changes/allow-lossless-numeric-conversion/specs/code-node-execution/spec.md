# Spec Delta

## MODIFIED Requirements

### Requirement: Evaluate typed CEL expressions through JSON ports

At execution, the shared runtime SHALL recursively validate each received JSON input against its declared Code port type
before invoking the node. Numeric input conversion SHALL use the shared lossless rules. The Code backend SHALL evaluate every declared output expression using the same input bindings
and recursively convert each CEL result to JSON without coercion. The shared runtime SHALL publish outputs only after
all evaluations and output type checks succeed. Collection values MUST have homogeneous elements or values according to
their declarations, and map keys MUST be strings. The node MUST enforce documented expression-length, JSON-size,
collection-size, and nesting-depth limits. Evaluation errors, nonrepresentable results, and limit violations SHALL fail
the node with the node ID, affected output name, and collection path where applicable. A branch-skipped Code node MUST
NOT evaluate expressions.

#### Scenario: Transform a value

- **WHEN** `amount` receives JSON integer `21` and `doubled` is `amount * 2`
- **THEN** the node produces JSON integer `42` on output `doubled`

#### Scenario: Reject a wrong input type

- **WHEN** `amount` is declared `int` but receives a JSON string
- **THEN** node execution fails before evaluating any expression

#### Scenario: Transform a typed list

- **WHEN** `items` is declared `{"list":"int"}`, receives `[1, 2]`, and `doubled` evaluates `items.map(x, x * 2)`
- **THEN** the node infers `doubled` as `List(Int64)` and produces `[2, 4]`

#### Scenario: Reject a heterogeneous collection

- **WHEN** a `{"list":"int"}` input contains a string, or a `{"map":"int"}` input contains a boolean value
- **THEN** node execution fails before evaluation and identifies the failing element or field

#### Scenario: Reject an over-limit collection

- **WHEN** an input or result exceeds the documented collection-size or nesting-depth limit
- **THEN** execution fails without publishing any Code output

#### Scenario: Publish atomically

- **WHEN** an output expression fails after another expression has produced a value
- **THEN** no Code outputs are published

#### Scenario: Skip an inactive node

- **WHEN** an upstream branch explicitly skips the Code node
- **THEN** no expression is evaluated and the existing skipped-output state propagates

#### Scenario: Convert exact numeric inputs

- **WHEN** an int input receives 42.0 or a double input receives integer 42
- **THEN** CEL receives the exact native value and emits the declared numeric representation

#### Scenario: Reject lossy numeric inputs

- **WHEN** an int input receives a fraction, negative zero, or an overflowing float, or a double input receives a nonrepresentable integer
- **THEN** execution fails before evaluating any expression
