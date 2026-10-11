## MODIFIED Requirements

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

### Requirement: Validate all startup values before execution

Every invocation SHALL validate its complete startup argument object before executing any node or consuming
any source data. It MUST reject duplicate keys, unknown qualified parameter names, invalid root object shapes, missing
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

### Requirement: Inspect an interface without running the workflow

The system SHALL expose startup interface version `2026-10-11` containing workflow identity, complete parameter declarations,
and runtime resource requirements. For compiled runners, direct manifest inspection SHALL return the
embedded build-time interface without constructing providers. Inspection MUST exclude argument values and node
configuration and MUST NOT execute nodes, consume source input, start timers, or emit execution events.
Invalid or incomplete metadata SHALL prevent launch.

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

- **WHEN** a launcher inspects a runner exposing an older startup interface version
- **THEN** manifest preflight rejects it before execution and directs the user to recompile the workflow
