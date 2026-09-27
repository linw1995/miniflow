# Spec Delta

## Purpose

Let a Flow perform concise, type-checked transformations through declared JSON ports while preserving an explicit language choice for future Code backends.

## ADDED Requirements

### Requirement: Select the Code package and language explicitly

The project SHALL provide an `mfn-code` package that registers `builtin.code`. A Flow MUST declare that package to use the kind. Every Code node MUST declare `language`; the first version SHALL accept only `cel` and MUST reject other language values with an unsupported-language diagnostic. Selecting `mfn-core` alone MUST NOT make `builtin.code` available.

#### Scenario: Select CEL

- **WHEN** a Flow declares `mfn-code` and a valid Code node with `language: "cel"`
- **THEN** the compiled runner resolves and validates that node without rebuilding the CLI

#### Scenario: Omit the package

- **WHEN** a Flow references `builtin.code` without a package providing it
- **THEN** compilation reports the existing unknown-kind diagnostic

#### Scenario: Request another language

- **WHEN** a Code node declares a language other than `cel`
- **THEN** build validation fails with the node ID and unsupported language

### Requirement: Declare typed CEL inputs and outputs

A CEL Code configuration SHALL require `language` , `inputs` , `outputs` , and `code` . `inputs` and `outputs` SHALL map
port names to types. `code` SHALL map each declared output name to one nonblank CEL expression, with no missing or extra
names. Input maps MAY be empty; output maps MUST be nonempty. Port names MUST be valid CEL identifiers. A type SHALL be
a concrete scalar `int` , `double` , `bool` , `string` , or `null` ; a recursively typed list such as `{"list":"int"}` ;
or a JSON object with string keys and one value type such as `{"map":"string"}` . These declarations SHALL become the
shared refined input and output port types. Dynamic, nullable-union, heterogeneous, and undeclared types MUST be
rejected. All declared ports SHALL be required. Unknown configuration fields MUST fail validation.

#### Scenario: Define an expression

- **WHEN** a Code node declares input `amount` as `int`, output `doubled` as `int`, and code `doubled: "amount * 2"`
- **THEN** its input and output ports use those names and the expression is checked against the declared types

#### Scenario: Resolve instance ports

- **WHEN** two Code nodes declare different input or output names
- **THEN** compilation checks each instance's edges and selected workflow outputs against its own refined port declarations

#### Scenario: Declare nested collections

- **WHEN** a port declares `{"list":{"map":"int"}}`
- **THEN** validation treats it as a list of JSON objects whose values are signed integers

#### Scenario: Reject incomplete declarations

- **WHEN** configuration has an invalid name or type, no outputs, blank expression, missing code expression, extra code expression, or unknown field
- **THEN** build validation fails with the node ID and affected field or output name

### Requirement: Type-check every expression before installation

The runner's validation mode SHALL parse and type-check every configured CEL expression against an environment
containing exactly the declared inputs. It MUST reject unknown identifiers, invalid operators or function calls,
unresolved dynamic types, and expression result types different from their declared output types. Validation MUST check
expressions on branches that could be skipped and MUST NOT evaluate expressions or execute workflow nodes. The
executable MUST NOT be installed after a validation failure.

#### Scenario: Reject an unknown input

- **WHEN** an expression uses a variable absent from `inputs`
- **THEN** compilation fails with the node ID and output expression context

#### Scenario: Reject a type mismatch

- **WHEN** an expression computes a `string` for an output declared `int`
- **THEN** compilation fails before executable installation

#### Scenario: Validate an inactive branch

- **WHEN** a Code node on a branch that would be skipped contains an invalid expression
- **THEN** compilation fails without executing that branch or any workflow node

### Requirement: Evaluate typed CEL expressions through JSON ports

At execution, the shared runtime SHALL recursively validate each received JSON input against its declared Code port type
before invoking the node. The Code backend SHALL evaluate every declared output expression using the same input bindings
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

- **WHEN** `items` is declared `{"list":"int"}`, receives `[1, 2]`, and an output declared `{"list":"int"}` evaluates `items.map(x, x * 2)`
- **THEN** the node produces `[2, 4]` on that output

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

### Requirement: Keep compiled workflows self-contained

The Code package SHALL compile CEL source into a checked program during node construction and evaluate that program
in-process. The compiled workflow binary SHALL include the required CEL evaluator and MUST run without its Flow
definition, Cargo, a language toolchain, or an external code execution service. Here, CEL compilation means parsing and
type checking into a checked expression; it does not mean native machine-code generation. The Code node's common port
and JSON result contract MUST be independent of the selected language.

#### Scenario: Run without build inputs

- **WHEN** a validated workflow binary containing a CEL Code node runs on a compatible host without its build inputs
- **THEN** it evaluates the embedded expressions and returns the selected outputs

#### Scenario: Rebuild after an expression edit

- **WHEN** a CEL expression changes in a reused build directory
- **THEN** the next compilation validates and installs behavior from the changed definition, preserving the old executable on failure
