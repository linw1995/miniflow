## ADDED Requirements

### Requirement: Fixed builtin port bags use the unified contract

Constant, Batch, and Readline SHALL express fixed named input and output bags with `NodeValue`. Migration SHALL retain configured Constant output refinements, shared payload identity, Batch's broad array metadata and flush behavior, and Readline's optional non-null path and stdin ownership. Configured-port providers such as IfElse, Loop, and Code SHALL retain dynamic interfaces.

#### Scenario: Preserve constant evidence and shared values

- **WHEN** a configured Constant produces a value consumed by Identity
- **THEN** literal type evidence is preserved and Identity retains the original shared value handle

#### Scenario: Preserve event and stream boundaries

- **WHEN** Batch accepts items or Readline receives an optional path
- **THEN** unified conversion preserves batching semantics and text source ownership without enabling task-only generated segments

### Requirement: Builtin Identity advertises certified generation

Identity SHALL expose a provider-owned constructor for its certified input and output fields. Eligible standard oneshot Identity chains SHALL use direct field moves through the shared lifecycle. Refined, observed, scoped, and stream paths SHALL retain the existing conservative dynamic fallback.

#### Scenario: Compile and execute an Identity chain

- **WHEN** a standard oneshot workflow connects sole-consumer Identity nodes and selects only the terminal output
- **THEN** generated execution uses an eligible typed segment and returns the same selected value as dynamic execution

#### Scenario: Retain configured refinement fallback

- **WHEN** a Constant establishes a concrete refinement upstream of an Identity chain
- **THEN** unsupported refinement proof retains dynamic execution and the configured workflow result remains unchanged
