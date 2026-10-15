# workflow-inputs Specification

## Purpose

Expose the input requirements of initial workflow nodes as a shared, typed startup interface for embedded
callers, standalone runners, and terminal launches.

## Requirements

### Requirement: Derive startup inputs from initial nodes

For schema `2026-10-03`, top-level nodes with no incoming data or control edges SHALL be initial nodes. Every
input port of an initial task or stream producer SHALL become a workflow startup parameter with its configured
type and required flag. Parameter bindings MUST NOT create graph dependencies or require a duplicate
user-authored input declaration.

#### Scenario: Start a parameterized producer

- **WHEN** an initial file producer declares a required string input named `path`
- **THEN** the workflow interface exposes that requirement and compilation succeeds without an upstream data
  edge or an actual path value

#### Scenario: Start an ordinary task with inputs

- **WHEN** an initial identity task declares a required input named `input`
- **THEN** the workflow exposes that input and can invoke the task once with a supplied startup value

#### Scenario: Derive configuration-dependent requirements

- **WHEN** two initial instances of one plugin kind expose different configured input ports
- **THEN** each instance contributes its own complete input contract

### Requirement: Preserve exact parameter identity

Startup values SHALL use a flat object keyed by the complete qualified name `node.port`. Interface
declarations SHALL preserve exact node and port identities. Same-named ports on different initial nodes MUST
remain distinct. Nodes without parameters SHALL retain an empty declaration and require no invocation value.
Preparation and manifest validation MUST reject declarations with colliding qualified names.

#### Scenario: Keep two path arguments separate

- **WHEN** initial nodes `read` and `write` both declare `path`
- **THEN** the caller supplies independent values under flat keys `read.path` and `write.path`

#### Scenario: Preserve punctuation in names

- **WHEN** a node ID or port name contains dots, slashes, or tildes
- **THEN** lookup matches the complete qualified name and diagnostics escape its JSON Pointer path

#### Scenario: Reject a qualified name collision

- **WHEN** initial node `a` declares `b.input` and initial node `a.b` declares `input`
- **THEN** preparation or manifest validation rejects the ambiguous name `a.b.input`

#### Scenario: Reject a nested argument document

- **WHEN** the caller supplies `{"read":{"path":"data.txt"}}` instead of `{"read.path":"data.txt"}`
- **THEN** validation rejects the unknown parameter `read` before execution

### Requirement: Keep noninitial and nested inputs explicit

Required inputs of nodes with incoming data or control edges SHALL still require data bindings. Unbound
optional inputs SHALL remain absent. Inputs of nested body nodes MUST NOT be promoted to the outer workflow;
their enclosing scope retains ownership. Context references MUST continue to require explicit upstream
dependencies.

#### Scenario: Reject a missing input behind a control edge

- **WHEN** a task is activated by a control edge but its required data input is unbound
- **THEN** compilation rejects the missing binding rather than creating a workflow parameter

#### Scenario: Keep body requirements local

- **WHEN** an initial Loop has a required outer variable and its body contains a node with an unbound required
  input
- **THEN** the outer variable becomes a startup parameter while the missing body binding still fails
  compilation

### Requirement: Validate all startup values before execution

Every invocation SHALL validate its complete startup argument object and resolve declared fallbacks before executing any node or consuming
node source data. It MUST reject duplicate keys, unknown qualified parameter names, invalid root object shapes, missing
required values, and type violations. Errors SHALL identify the affected qualified parameter and nested path without
coercion.

#### Scenario: Reject one invalid argument across multiple roots

- **WHEN** one initial node has valid arguments and another is missing a required argument
- **THEN** startup fails before either node executes or any stream producer reads data

#### Scenario: Reject ambiguous JSON arguments

- **WHEN** an argument document repeats a qualified parameter key or a key within its value
- **THEN** validation rejects it rather than keeping either occurrence

#### Scenario: Reject a nested type mismatch

- **WHEN** a startup parameter declared as a list of integer maps contains a string at `/0/count`
- **THEN** startup fails with the node, port, expected type, and nested mismatch path

### Requirement: Preserve omission and null semantics

An omitted optional parameter without a configured fallback SHALL remain absent from node inputs. Explicit null SHALL be a supplied value
subject to its port type. The startup interface MUST NOT infer undeclared defaults, coerce values, or derive later
invocation schemas from values supplied by earlier runs.

#### Scenario: Omit an optional port

- **WHEN** the caller omits an optional string parameter without a fallback
- **THEN** the node receives no binding for that port

#### Scenario: Supply null to a string port

- **WHEN** the caller supplies null for a string parameter
- **THEN** startup rejects the value even if the port is optional

### Requirement: Bind parameters once per workflow invocation

Startup values SHALL be isolated by workflow invocation and bound once to their initial nodes. Emitted stream
messages MUST NOT rebind initial nodes or implicitly broadcast startup outputs into other message domains.
Single-run and stream entry points SHALL use the same parameter validation and binding rules.

#### Scenario: Produce many records from one path

- **WHEN** an initial producer receives a file path and emits many records
- **THEN** it receives the path once and each record independently activates its downstream message processing

#### Scenario: Invoke the same plan with different parameters

- **WHEN** two workflow instances supply different arguments to the same initial node
- **THEN** each instance observes only its own startup values and neither changes the other's input contract

### Requirement: Inspect an interface without running the workflow

The startup interface SHALL include workflow identity, complete port/resource declarations,
and binding metadata with literal defaults. Its version SHALL be `2026-10-16` for bindings
and `2026-10-11` otherwise. Inspection SHALL read embedded metadata without constructing
providers, reading sources, executing nodes, starting timers, or emitting events. Invocation
values, resolved environment values, and unrelated configuration MUST be excluded.
Invalid metadata MUST prevent launch.

#### Scenario: Inspect an external producer's dynamic ports

- **WHEN** a compiled workflow is inspected without its build inputs or the referenced input file
- **THEN** its interface reports the producer's configured input requirements without constructing providers or opening that file

#### Scenario: Reject mismatched interface metadata

- **WHEN** a launcher receives graph and interface records identifying different workflows
- **THEN** it rejects preflight before workflow execution

#### Scenario: Keep inspection independent of factory initialization

- **WHEN** a new runner's factory would require an unavailable runtime resource to initialize an executor
- **THEN** inspection still returns the embedded interface without initializing that executor, while execution retains its ordinary construction failure behavior

#### Scenario: Reject the previous nested-argument interface

- **WHEN** a launcher inspects a runner exposing an unsupported older startup interface version
- **THEN** manifest preflight rejects it before execution and directs the user to recompile the workflow

### Requirement: Reject generated startup interface drift

New generated runners SHALL compare their freshly prepared startup schema with the embedded interface during every execution invocation. Initial-node and port identities, types, required flags, binding rules, and resource conditions MUST agree. Disagreement SHALL fail before executor invocation, source consumption, timers, or worker scheduling, with affected node/port/resource context. Dynamic in-memory workflows SHALL retain their existing preparation rules.

#### Scenario: Reject a changed input type

- **WHEN** a generated runner prepares an initial input with a type different from its embedded declaration
- **THEN** execution fails with the affected node and port before any node executes

#### Scenario: Reject a changed required flag or port set

- **WHEN** runtime preparation changes an input's required flag, adds or removes a port, or changes the initial-node input declarations
- **THEN** the runner rejects the mismatch rather than executing with a different interface

#### Scenario: Reject changed stdin ownership

- **WHEN** runtime preparation changes unconditional or conditional stdin ownership from the embedded declaration
- **THEN** the runner rejects the mismatch before reading source data or starting workers

#### Scenario: Preserve an installed runner after validation mismatch

- **WHEN** a newly built runner's validation detects an interface mismatch before installation
- **THEN** compilation fails and preserves the previously installed executable

#### Scenario: Accept matching task and stream interfaces

- **WHEN** a generated task or stream runner prepares the same schema as its embedded interface
- **THEN** it continues with ordinary authoritative argument/resource validation and unchanged execution semantics

#### Scenario: Keep dynamic embedded callers independent

- **WHEN** an in-memory caller prepares a workflow without a generated executable
- **THEN** ordinary dynamic interface derivation and argument validation work without requiring an embedded manifest

#### Scenario: Reject changed binding rules

- **WHEN** runtime preparation changes an environment name, default value, or secret flag
- **THEN** normal and isolated execution reject the mismatch before source resolution or node invocation

### Requirement: Describe runner arguments without execution

New compiled runners SHALL accept `--help` and `-h`, write schema-specific help to stdout, and exit with
status 0. Help SHALL list qualified inputs, types, required flags, JSON transports, and verbosity. With
`--node ID`, it SHALL show only the selected task's inputs. Help MUST NOT require input values, read input
or context files, construct providers, initialize telemetry, or execute nodes.

#### Scenario: Inspect missing required inputs

- **WHEN** a runner receives `--help` without its required startup inputs
- **THEN** it lists input contracts and exits successfully without preparing or executing nodes

#### Scenario: Inspect a selected task without context files

- **WHEN** a runner receives `--node transform --context-file missing.json --help`
- **THEN** help describes only that task's inputs and does not open the context file

### Requirement: Parse runner CLI syntax consistently

CLI syntax errors SHALL write diagnostics to stderr and exit with status 2. Domain validation failures
SHALL retain typed diagnostics and a nonzero status. Input transports SHALL accept separate and inline
values while rejecting duplicates and mixing with named inputs. Short-option-looking named values SHALL
remain literal; values starting with `--` SHALL require inline syntax. Clap's standard end-of-options
marker SHALL be accepted, followed by ordinary schema validation.

#### Scenario: Reject unknown runner syntax

- **WHEN** a runner receives an unknown CLI option
- **THEN** it writes a CLI diagnostic to stderr and exits with status 2 before business execution

#### Scenario: Reject individually valid mixed transports

- **WHEN** valid named inputs are combined with a valid JSON input document
- **THEN** the runner rejects the combination instead of selecting one transport

#### Scenario: Preserve option-looking literal values

- **WHEN** a string parameter receives `-v` as its following value or `--verbose` as an inline value
- **THEN** the supplied value remains literal input data rather than a verbosity option

### Requirement: Resolve declared startup sources consistently

Startup input bindings MAY declare an environment name, a typed JSON default, and a
secret flag. Explicit arguments SHALL take precedence over environment values, then
defaults. A missing required value or an invalid present value MUST fail before node
execution without selecting a lower-priority source. Finite, streaming, embedded,
and isolated task execution SHALL share these rules.

#### Scenario: Override an environment value

- **WHEN** the caller supplies an argument and its configured environment variable is present
- **THEN** execution uses the explicit argument without reading the lower-priority source

#### Scenario: Use an environment value or default

- **WHEN** no explicit argument is supplied
- **THEN** execution uses the named environment value if present, otherwise the configured default

#### Scenario: Do not treat empty text as missing

- **WHEN** a string parameter has a default but its environment variable is an empty string
- **THEN** execution supplies the empty string rather than the default

#### Scenario: Reject invalid present values

- **WHEN** a supplied argument or environment value violates the declared type
- **THEN** execution fails without selecting a lower-priority source or starting any node

#### Scenario: Rebind a reused execution context

- **WHEN** a context is reused for a workflow with a different default and no explicit argument
- **THEN** the new invocation resolves the new default instead of retaining the previous bound value

### Requirement: Decode bounded environment input

Only named environment variables SHALL be read, with an aggregate 1 MiB text budget.
Non-Unicode values MUST fail without raw payloads in diagnostics. Strings SHALL retain
their text; other types SHALL use the named argument JSON codec. Preparation, manifest
inspection, help, and argument parsing MUST NOT read the environment.

#### Scenario: Reject invalid environment text

- **WHEN** a selected environment value is non-Unicode or not valid JSON for its declared type
- **THEN** binding fails without falling back to a configured default

#### Scenario: Preserve inspection independence

- **WHEN** help or manifest inspection is requested with environment values unavailable
- **THEN** declarations remain inspectable without resolving those sources

### Requirement: Mark secret startup values before observation

The shared binder SHALL mark values for secret input bindings before any node input
snapshot, regardless of their source. Explicit argument parsing SHALL preserve the
same marking policy. Public bindings MUST NOT remove an existing secret marker.
Error diagnostics MUST NOT include raw environment values or duplicate JSON key text.
Business access and serialization SHALL retain original values.

#### Scenario: Protect each input source

- **WHEN** a secret parameter is supplied by arguments, environment, or a default
- **THEN** its first input snapshot is redacted and its business value remains available

#### Scenario: Reject duplicate secret object keys safely

- **WHEN** secret environment JSON repeats a sensitive object key
- **THEN** parsing rejects the duplicate without including that key in the error chain
