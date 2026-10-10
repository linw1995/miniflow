## MODIFIED Requirements

### Requirement: Fixed builtin port bags use the unified contract

Constant, Batch, and Readline SHALL use `NodeValue` bags and typed execution contracts. Their factories MUST NOT
handwrite `NodePortContract` or override ports. Constant SHALL preserve literal evidence and shared values. Batch SHALL
reflect `List(Any)` before collection inference and retain flush behavior. Readline SHALL preserve its optional non-null
path and stdin ownership. IfElse, Loop, and Code SHALL reflect validated dynamic contracts.

#### Scenario: Preserve constant evidence and shared values

- **WHEN** a configured Constant produces a value consumed by Identity
- **THEN** literal type evidence is preserved and Identity retains the original shared value handle

#### Scenario: Preserve event and stream boundaries

- **WHEN** Batch accepts items or Readline receives an optional path
- **THEN** runtime-owned typed conversion preserves batching semantics and text source ownership without enabling task-only generated segments

#### Scenario: Keep fixed declarations independent of output evidence

- **WHEN** Constant, Batch, or Readline is prepared without invocation values
- **THEN** its port names, field descriptors, and requiredness come from the declared value bags while evidence is preserved separately

#### Scenario: Change a fixed execution field

- **WHEN** a fixed builtin changes a field in its execution-associated input or output value type
- **THEN** preparation reflects the corresponding contract without updating a separate port declaration
