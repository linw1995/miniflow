# Spec Delta

## ADDED Requirements

### Requirement: Generate proven typed serial task segments

Standard oneshot binaries SHALL generate direct typed calls and field wiring for eligible serial task segments inside
one existing top-level execution domain. Eligibility SHALL require provider generation support and proven type, codec,
presence, ownership, and observation compatibility. Internal connections MUST NOT construct dynamic maps or round-trip
their values through dynamic encoding and decoding.

#### Scenario: Compile a typed chain

- **WHEN** linked providers form an eligible serial chain with a required owned field passed to one consumer
- **THEN** the binary directly constructs typed inputs and invokes the providers without internal dynamic conversion

#### Scenario: Map differently named struct fields

- **WHEN** an eligible edge connects differently named source and target ports with identical Rust field types
- **THEN** generated wiring follows the declared edge mapping without requiring identical aggregate structs

#### Scenario: Preserve dynamic segment boundaries

- **WHEN** an eligible typed segment receives startup or dynamic inputs and produces a selected workflow output
- **THEN** its entry decodes dynamic bindings and its exit encodes the selected result while internal edges remain typed

### Requirement: Select typed generation deterministically with safe fallback

Compilation SHALL retain dynamic execution for unsupported providers, optional internal bindings, unproven codecs or
refinements, forks, joins, observed values, cross-domain edges, streams, nested bodies, and custom-runner context
inspection. It SHALL expose deterministic eligibility or fallback reasons in build inspection data. Missing optional
support MUST NOT invalidate an otherwise valid workflow.

#### Scenario: Keep a mixed-provider workflow valid

- **WHEN** a valid workflow combines typed-generation providers with a dynamic configuration-dependent provider
- **THEN** compilation emits typed segments where proven and retains dynamic boundaries around the unsupported provider

#### Scenario: Keep a shared output available

- **WHEN** an output has a second data consumer, control reader, context reference, or selected workflow-output use
- **THEN** compilation does not move the value into a sole-consumer fast path that would invalidate the other observer

#### Scenario: Retain optional internal presence handling

- **WHEN** a serial connection uses an optional source or target field
- **THEN** the first-version planner retains dynamic handling and records its fallback reason

#### Scenario: Preserve custom-runner inspection

- **WHEN** a caller generates general-purpose artifacts and inspects intermediate outputs through the final context
- **THEN** those outputs retain their existing dynamic availability

### Requirement: Resolve typed generation through selected provider packages

The generated Cargo build SHALL obtain typed generation information from the same selected providers, configuration,
features, and package identities used for ordinary validation. References SHALL support dependency aliases and target
compilation. Invalid advertised contracts or generated type assignments SHALL fail before installation with retained
Cargo diagnostics. Build validation MUST NOT invoke business logic.

#### Scenario: Compile an external aliased provider

- **WHEN** a Flow selects an external provider through a dependency alias and the provider exports a typed shim
- **THEN** the installed CLI compiles its typed segment without knowing the provider implementation or exposing private
  executor types

#### Scenario: Reject an incorrect Rust identity claim

- **WHEN** a provider advertises matching field identities but the target runner cannot type-check the generated
  assignment
- **THEN** compilation fails before installation and preserves the previously installed executable and relevant
  diagnostics

#### Scenario: Regenerate after provider changes

- **WHEN** provider source, configuration, features, or generation descriptors change in a reused build directory
- **THEN** the current Cargo build regenerates and validates typed segments rather than reusing stale eligibility
  decisions

#### Scenario: Retain inspection without initialization

- **WHEN** a typed-generated binary is inspected without execution
- **THEN** its existing embedded graph and startup manifest remain available without constructing executors or requiring
  generation sidecars
