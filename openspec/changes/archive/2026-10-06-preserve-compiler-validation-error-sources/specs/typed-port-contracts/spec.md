## ADDED Requirements

### Requirement: Preserve typed compiler validation sources

Compiler port and derivation validation failures SHALL retain their original typed error sources and source chains, together with node and port context. Diagnostic formatting SHALL NOT replace the underlying errors with strings.

#### Scenario: Preserve a contradictory literal's nested mismatch

- **WHEN** output derivation validation rejects a literal element that contradicts the declared collection type
- **THEN** the compiler error identifies the node and retains both `OutputDerivationError` and its `TypeMismatch` source with the failing JSON Pointer

#### Scenario: Preserve a forwarded known-value mismatch

- **WHEN** an exact forwarded value contradicts its output declaration
- **THEN** the compiler error identifies the node and output and retains the `TypeMismatch` source

#### Scenario: Preserve excessive port depth

- **WHEN** a declared port type exceeds the shared nesting limit
- **THEN** the compiler error identifies the node, port, and direction and retains the `TypeDepthError` source
