# code-node-execution Specification

## Purpose

Let a Flow perform concise, type-checked transformations through declared JSON ports while preserving an explicit language choice for future Code backends.

## Requirements

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

### Requirement: Declare CEL inputs and infer output ports

A CEL Code configuration SHALL require `language`, `inputs`, and `code`. `inputs` SHALL map input names to declared
types; `code` SHALL map output names to nonblank CEL expressions. Input maps MAY be empty; code maps MUST be nonempty.
Port names MUST be valid CEL identifiers. Input types SHALL accept `int`, `uint`, `double`, `bool`, `string`, `null`,
`any`, `object`, `array`, and recursively typed lists and string-keyed maps. Object fields, array elements, and any
values MAY contain heterogeneous JSON. The CEL checker SHALL infer each output type from its expression, preserving
concrete collection structure where known and using dynamic JSON types where necessary. Declared input types and
inferred output types SHALL become shared refined port types. Nullable-union and undeclared input types MUST be
rejected. All ports SHALL be required. Unknown configuration fields MUST fail validation.

#### Scenario: Define an expression

- **WHEN** a Code node declares input `amount` as `int` and code `doubled: "amount * 2"`
- **THEN** it exposes input `amount` as `Int64` and infers output `doubled` as `Int64`

#### Scenario: Resolve instance ports

- **WHEN** two Code nodes declare different input names or output expression names
- **THEN** compilation checks each instance's edges and selected workflow outputs against its own refined port declarations

#### Scenario: Declare nested collections

- **WHEN** an input declares `{"list":{"map":"int"}}`
- **THEN** validation treats it as a list of JSON objects whose values are signed integers

#### Scenario: Reject incomplete declarations

- **WHEN** configuration has an invalid input name or type, no output expressions, a blank expression, or an unknown field
- **THEN** build validation fails with the node ID and affected field or output name

#### Scenario: Concatenate structured object lists

- **WHEN** `left` and `right` declare `{"list":"object"}` and output `value` evaluates `left + right`
- **THEN** the inferred output is `List(Map(Any))` and can connect to a `List(Object)` consumer

#### Scenario: Declare broad JSON inputs

- **WHEN** inputs declare `any`, `object`, or `array`
- **THEN** they accept any JSON value, only JSON objects, or only JSON arrays respectively, including nested heterogeneous content

### Requirement: Type-check every expression before installation

The generated Cargo build script SHALL parse and type-check every configured CEL expression against an environment
containing exactly the declared inputs. It MUST reject unknown identifiers, invalid operators or function calls,
inferred result types that the shared JSON port contract cannot represent, and statically known non-string map keys.
Explicit `dyn(...)` calls and dynamic result types SHALL be accepted with runtime JSON validation.
It SHALL derive output port types from the checked results. Validation MUST check expressions on branches
that could be skipped and MUST NOT evaluate expressions or execute workflow nodes. The executable MUST NOT be installed
after a validation failure.

#### Scenario: Reject an unknown input

- **WHEN** an expression uses a variable absent from `inputs`
- **THEN** compilation fails with the node ID and output expression context

#### Scenario: Reject an invalid operation

- **WHEN** an expression adds an `int` input to a string literal
- **THEN** compilation fails before executable installation

#### Scenario: Reject an unsupported inferred type

- **WHEN** an expression infers bytes or another type that cannot be represented as a shared JSON port type
- **THEN** compilation fails with the node ID and output expression name

#### Scenario: Validate an inactive branch

- **WHEN** a Code node on a branch that would be skipped contains an invalid expression
- **THEN** compilation fails without executing that branch or any workflow node

#### Scenario: Use an explicit dynamic expression

- **WHEN** an expression evaluates `dyn(value)` for a declared input
- **THEN** construction accepts it with an Any output and execution validates the produced JSON value

#### Scenario: Validate a dynamic downstream binding

- **WHEN** an Any Code output connects to a concrete input
- **THEN** the connection retains runtime validation and incompatible produced values fail before the consumer runs

### Requirement: Evaluate typed CEL expressions through JSON ports

At execution, the shared runtime SHALL recursively validate each received JSON input against its declared Code port type
before invoking the node. Numeric input conversion SHALL use the shared lossless rules. The Code backend SHALL evaluate every declared output expression using the same input bindings
and recursively convert each CEL result to JSON without coercion. The shared runtime SHALL publish outputs only after
all evaluations and output type checks succeed. Collection values MUST satisfy their declared element or value types,
including heterogeneous values permitted by dynamic declarations, and map keys MUST be strings. The node MUST enforce documented expression-length, JSON-size,
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

#### Scenario: Preserve structured list contents

- **WHEN** `left + right` combines object lists containing nested node paths, absent token fields, nulls, and duplicate call IDs
- **THEN** the output preserves every entry in left-then-right order without inserting fields, deduplication, or aggregation

#### Scenario: Concatenate empty and heterogeneous lists

- **WHEN** two List(Any) inputs contain empty lists or mixed null, scalar, object, and array elements
- **THEN** concatenation preserves their values and order

#### Scenario: Fail a dynamic operation

- **WHEN** dynamically typed operands do not support the evaluated operation
- **THEN** execution fails with output attribution and publishes no Code outputs

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

### Requirement: Explicitly convert numeric text

CEL Code SHALL accept `int(text)` and `double(text)` for declared string inputs and infer Int64 and
Float64 output ports respectively. Integers SHALL fit signed 64-bit range. Floating-point
conversion SHALL accept decimal and scientific notation and produce a finite JSON number. Invalid text,
integer overflow, or a nonfinite result MUST fail execution without publishing Code outputs. Conversion SHALL be explicit; input ports retain strict type checks.

#### Scenario: Convert a text integer

- **WHEN** string input `text` is `"42"` and the expression is `int(text)`
- **THEN** the node emits JSON integer 42 through an inferred Int64 port

#### Scenario: Convert a text double

- **WHEN** string input `text` is `"1e3"` and the expression is `double(text)`
- **THEN** the node emits finite JSON floating-point value 1000.0 through an inferred Float64 port

#### Scenario: Reject invalid numeric text

- **WHEN** conversion receives invalid text, an overflowing integer, or a nonfinite floating-point value
- **THEN** execution fails without publishing Code outputs

### Requirement: Preserve numeric values across dynamic JSON conversion

Dynamic JSON conversion SHALL preserve signed and unsigned 64-bit integers without floating-point coercion, and
preserve stored floating-point values including negative zero. UInt results SHALL use unsigned JSON integers.
Declared numeric inputs SHALL retain the existing lossless conversion rules.

#### Scenario: Round trip large integers

- **WHEN** an object or Any value contains integers above the exact double range, signed minimum, or unsigned maximum
- **THEN** an identity expression or list concatenation returns the exact integer values

#### Scenario: Preserve floating representation

- **WHEN** dynamic JSON contains 42.0 or negative floating zero
- **THEN** passing the value through Code preserves floating representation and the zero sign

#### Scenario: Validate unsigned input

- **WHEN** a uint input receives a negative number, fractional number, negative zero, or out-of-range value
- **THEN** execution rejects it using the existing lossless unsigned conversion rules

### Requirement: Reject non-JSON dynamic results

Dynamic result conversion SHALL accept only JSON-representable values recursively. It MUST reject non-finite numbers,
non-string object keys, non-JSON CEL values, and embedded evaluation errors without coercing them to strings or null.
Failures SHALL identify the output and nested JSON Pointer where applicable and prevent all output publication.

#### Scenario: Reject a nested non-JSON value

- **WHEN** a dynamic result contains bytes, a timestamp, a duration, a non-finite number, or an embedded evaluation error
- **THEN** conversion fails with output and applicable nested-path context, publishing no output

#### Scenario: Reject dynamic non-string keys

- **WHEN** a map with a non-string key is hidden behind a dynamic result type
- **THEN** execution rejects that map instead of stringifying its keys

### Requirement: Propagate Code input sensitivity explicitly

Code SHALL mark every produced output secret when any supplied input contains a secret, including nested secrets.
This provider policy SHALL apply to concrete and dynamic expressions in direct and workflow execution. Business JSON
values SHALL remain unchanged, and observations SHALL use existing redaction. Inputs without secrets MUST NOT become
secret solely because they pass through Code.

#### Scenario: Transform a secret descendant

- **WHEN** an object input contains a secret descendant and Code extracts or computes a scalar from it
- **THEN** the produced output is secret and existing observations redact it

#### Scenario: Concatenate secret and public entries

- **WHEN** one supplied list contains a secret entry and another list is public
- **THEN** the complete concatenated output is secret while business reads retain all original entries

#### Scenario: Apply the conservative policy to independent expressions

- **WHEN** any supplied input contains a secret and another output expression is constant
- **THEN** that output is also marked secret without requiring expression dependency analysis

#### Scenario: Preserve public output

- **WHEN** all supplied inputs are public
- **THEN** ordinary Code outputs remain public

### Requirement: Bound dynamic JSON traversal

Dynamic descendants SHALL obey the same limits as concrete values: 8 KiB per expression, 1 MiB each for serialized
inputs and outputs, 10,000 collection entries across input and output conversion combined, and 16 value nesting
levels. Broad type declarations MUST NOT bypass traversal limits. A limit failure SHALL publish no Code outputs.

#### Scenario: Bound deep dynamic content

- **WHEN** an Any or Object input hides a value exceeding the depth or collection-entry limit
- **THEN** execution rejects it with applicable path context

#### Scenario: Bound repeated accumulation

- **WHEN** repeated list concatenation crosses a documented input, output, or combined conversion limit
- **THEN** that invocation fails without publishing a truncated list or a successful partial result
