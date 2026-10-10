## MODIFIED Requirements

### Requirement: Expose optional output derivation metadata

Nodes MAY declare that an output is an exact configured JSON value, an unchanged copy of a named input, a collection of values from a named input, or a type proven by a prepared contract.
Validation SHALL use such declarations only for the node's existing ports and MUST reject malformed derivation
metadata or an exact value that contradicts its own output declaration, with node and port context. Nodes that
provide no derivation metadata SHALL retain their reflected port types and existing
runtime boundary checks. Execution SHALL continue to validate actual produced and bound values against the
resolved ports.

A collection derivation SHALL wrap the input type as `List(T)`, retain dynamic boundary checks for broad elements, and discard exact-value evidence for the collected output. It MUST reference an existing input and output, fit the output declaration, and respect the shared type-depth limit. Generated and in-memory preparation SHALL use the same derivation rules without node-kind-specific inference.

#### Scenario: Preserve an ordinary plugin

- **WHEN** a plugin declares an `Any` output and supplies no output derivation
- **THEN** a connection to an `Int64` input remains runtime-checked

#### Scenario: Reject an invalid derivation

- **WHEN** a plugin derives an output from a nonexistent input port
- **THEN** build validation fails before installation and identifies the plugin node and invalid port reference

#### Scenario: Reject a contradictory literal declaration

- **WHEN** a plugin declares a `String` output but derives it from an exact integer literal
- **THEN** build validation rejects the node metadata before installation

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

A proven-type derivation SHALL narrow the descriptor of an existing reflected output without changing its name or
requiredness. It SHALL carry type evidence without an exact value and MUST respect the shared descriptor-depth limit.
Compilation and runtime publication SHALL enforce it through the existing validation path.

#### Scenario: Resolve a type proven by a prepared body

- **WHEN** an output field reflects `List(Any)` and its prepared body proves `List(Int64)`
- **THEN** compiler inference resolves `List(Int64)` without treating the output as an exact known array

#### Scenario: Reject widening type evidence

- **WHEN** an `Int64` output supplies proven-type evidence for `Any` or `String`
- **THEN** validation rejects the evidence with node and output context

#### Scenario: Guard a false proven type

- **WHEN** an output proves `Int64` but business execution returns a string
- **THEN** runtime publication rejects the result before exposing any output

### Requirement: Fixed builtin port bags use the unified contract

Constant, Batch, and Readline SHALL express fixed named input and output bags with `NodeValue`. Their raw ports SHALL reflect the value bags without factory overrides. Constant SHALL retain literal type evidence and shared payload identity. Batch SHALL reflect `List(Any)` before collection inference while preserving flush behavior. Readline SHALL retain its optional non-null path and stdin ownership. IfElse, Loop, and Code SHALL reflect validated dynamic execution contracts.

#### Scenario: Preserve constant evidence and shared values

- **WHEN** a configured Constant produces a value consumed by Identity
- **THEN** literal type evidence is preserved and Identity retains the original shared value handle

#### Scenario: Preserve event and stream boundaries

- **WHEN** Batch accepts items or Readline receives an optional path
- **THEN** unified conversion preserves batching semantics and text source ownership without enabling task-only generated segments

#### Scenario: Keep fixed declarations independent of output evidence

- **WHEN** Constant, Batch, or Readline is prepared without invocation values
- **THEN** its port names, field descriptors, and requiredness come from the declared value bags while evidence is preserved separately
