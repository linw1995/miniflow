# parameters-node-execution Specification

## Purpose

Provide typed workflow parameters from invocation arguments, named environment variables,
and defaults, with source-independent secret protection and ordinary graph outputs.

## Requirements

### Requirement: Declare typed parameter inputs and outputs

The `mfn-core` package SHALL register `builtin.parameters`. Its configuration SHALL
contain a nonempty `parameters` object keyed by nonempty parameter names. Each entry
SHALL declare a supported port `type`, MAY declare `env` and a typed JSON `default`,
and SHALL default `secret` to true. Unknown fields and invalid types or defaults MUST
fail preparation. Each parameter SHALL expose one required input and one required
output with the same name and configured type.

#### Scenario: Prepare without credentials

- **WHEN** a parameter references an environment variable that is unavailable during build
- **THEN** preparation succeeds without reading the variable and reflects the configured ports

#### Scenario: Reject an invalid default

- **WHEN** an integer parameter configures a string or null default
- **THEN** preparation rejects the type mismatch without executing the node

#### Scenario: Preserve a nullable default

- **WHEN** a nullable string parameter explicitly configures a null default
- **THEN** the binding retains that default through interface serialization and deserialization

### Requirement: Keep parameter declarations at workflow startup

Parameter nodes SHALL be limited to top-level initial nodes with no incoming data or
control edges. Nested Loop and Iteration bodies and legacy definitions without startup
parameters MUST NOT host parameter bindings.

#### Scenario: Reject a parameter behind an edge

- **WHEN** a parameter node has an incoming data or control edge
- **THEN** compilation rejects it instead of resolving configuration during graph execution

### Requirement: Forward bound parameter values

Parameter execution SHALL forward the input values supplied by the shared startup
binder to their corresponding outputs. It MUST preserve JSON values, shared payloads,
and secret metadata. It MUST NOT resolve sources again or remove an existing secret
marker when the parameter is configured as public.

#### Scenario: Route a secret credential

- **WHEN** a secret parameter supplies a credential to an identity node
- **THEN** both nodes retain the original business value and their input/output snapshots are redacted

#### Scenario: Preserve a caller-provided secret on a public parameter

- **WHEN** an embedded caller supplies a marked secret to a parameter with secret disabled
- **THEN** forwarding preserves the existing marker and shared value identity
