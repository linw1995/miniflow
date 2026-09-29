# Spec Delta

## ADDED Requirements

### Requirement: Compile structured Loop definitions into executable binaries

The system SHALL provide a compilation entry point that reads supported date-versioned workflow
definitions and produces an executable workflow binary. It MUST reject malformed or unsupported
versions. The binary MUST encode validated scope-local node orders and port bindings as generated
executable code, including structured repeated execution for Loop bodies, and require no source
definition file at runtime. An installed CLI SHALL compile a project with declared third-party nodes
without rebuilding the CLI or requiring a miniflow source checkout. Compilation SHALL require Cargo,
a compatible Rust toolchain, available versioned support packages, and the declared dependencies.
The executable MUST run without the dependency lock, Cargo, or plugin source directories.
Definitions in the earlier `2026-09-26` version SHALL retain their existing DAG behavior.

#### Scenario: Compile a valid workflow

- **WHEN** a user compiles a valid DAG definition or a valid structured Loop definition that references registered ordinary node kinds
- **THEN** the system generates and compiles a runner with the planned scope-local order

#### Scenario: Run a compiled Loop workflow

- **WHEN** a user runs a generated binary containing a Loop
- **THEN** the binary repeats its body according to the Loop contract and returns the same selected outputs as in-memory execution

#### Scenario: Run without build inputs

- **WHEN** the generated executable is moved to a compatible runtime environment without its project files, plugin sources, or Rust build tools
- **THEN** it executes its embedded DAG or Loop workflow and produces the selected outputs

### Requirement: Validate every Loop scope before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs within each scope,
existing local edge endpoints, selected output node references and names, scope boundaries, reserved
engine constructs, Loop count and nesting limits, and acyclicity within every graph. Before
installing the executable, its validation mode SHALL validate ordinary registered kinds,
configuration, existing ports, required input connections, typed Loop variables, assignments,
termination conditions, and context references in every scope. Validation MUST NOT execute node
implementations. A failed validation MUST preserve any previously installed output binary.

#### Scenario: Reject a body cycle before code generation

- **WHEN** a Loop body has a data or control cycle
- **THEN** structural planning fails with a cycle path in that body before generating the runner

#### Scenario: Reject an invalid body plugin before installation

- **WHEN** a body node has an unknown kind or invalid configuration
- **THEN** runner validation fails before installation without running that node

#### Scenario: Preserve an existing binary after invalid Loop edit

- **WHEN** a previously valid Loop definition is edited into an invalid one and rebuilt
- **THEN** the build fails while preserving the installed executable and adjacent dependency lock
