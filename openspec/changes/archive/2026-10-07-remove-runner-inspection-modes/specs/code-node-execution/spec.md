## MODIFIED Requirements

### Requirement: Type-check every expression before installation

The generated Cargo build script SHALL parse and type-check every configured CEL expression against an environment
containing exactly the declared inputs. It MUST reject unknown identifiers, invalid operators or function calls,
explicit `dyn(...)` calls, dynamic result types, and inferred result types that the shared JSON port contract cannot
represent. It SHALL derive output port types from the checked results. Validation MUST check expressions on branches
that could be skipped and MUST NOT evaluate expressions or execute workflow nodes. The executable MUST NOT be installed
after a validation failure.

#### Scenario: Reject an unknown input

- **WHEN** an expression uses a variable absent from `inputs`
- **THEN** compilation fails with the node ID and output expression context

#### Scenario: Reject an invalid operation

- **WHEN** an expression adds an `int` input to a string literal
- **THEN** compilation fails before executable installation

#### Scenario: Reject an unsupported inferred type

- **WHEN** an expression infers `dyn`, bytes, or another type that cannot be represented as a shared JSON port type
- **THEN** compilation fails with the node ID and output expression name

#### Scenario: Validate an inactive branch

- **WHEN** a Code node on a branch that would be skipped contains an invalid expression
- **THEN** compilation fails without executing that branch or any workflow node
