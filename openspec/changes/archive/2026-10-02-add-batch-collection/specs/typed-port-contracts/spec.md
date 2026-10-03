# Spec Delta

## MODIFIED Requirements

### Requirement: Expose optional output derivation metadata

Nodes MAY declare that an output is an exact configured JSON value, an unchanged copy of a named input, or a collection of values from a named input.
Validation SHALL use such declarations only for the node's existing ports and MUST reject malformed derivation
metadata or an exact value that contradicts its own output declaration, with node and port context. Nodes that
provide no derivation metadata SHALL retain their declared or configuration-derived port types and existing
runtime boundary checks. Execution SHALL continue to validate actual produced and bound values against the
resolved ports.

A collection derivation SHALL wrap the input type as `List(T)`, retain dynamic boundary checks for broad elements, and discard exact-value evidence for the collected output. It MUST reference an existing input and output, fit the output declaration, and respect the shared type-depth limit. Generated and in-memory preparation SHALL use the same derivation rules without node-kind-specific inference.

#### Scenario: Preserve an ordinary plugin

- **WHEN** a plugin declares an `Any` output and supplies no output derivation
- **THEN** a connection to an `Int64` input remains runtime-checked

#### Scenario: Reject an invalid derivation

- **WHEN** a plugin derives an output from a nonexistent input port
- **THEN** runner validation fails before installation and identifies the plugin node and invalid port reference

#### Scenario: Reject a contradictory literal declaration

- **WHEN** a plugin declares a `String` output but derives it from an exact integer literal
- **THEN** runner validation rejects the node metadata before installation

#### Scenario: Guard a false derivation

- **WHEN** a plugin declares an output derivation but produces a value outside its resolved output type
- **THEN** execution fails before publishing that output

#### Scenario: Derive a collection element type

- **WHEN** a node declares collection-of-input evidence for an input bound to `Int64`
- **THEN** its output resolves to `List(Int64)` without a known exact array value

#### Scenario: Keep unknown elements checked

- **WHEN** an input bound to `Any` is collected and its output feeds `List(Int64)`
- **THEN** the connection retains runtime element checks and an invalid element reports its array index

#### Scenario: Reject excessive derived nesting

- **WHEN** wrapping the bound input type in a list exceeds the descriptor depth limit
- **THEN** validation fails with the deriving node and output port
