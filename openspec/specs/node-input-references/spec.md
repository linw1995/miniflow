# node-input-references Specification

## Purpose

Define node-local data bindings that normalize into a complete dependency graph while preserving types, scope, exclusivity, and execution semantics.

## Requirements

### Requirement: Use one binding syntax in every scope

All supported versions SHALL use consumer-local references for data inputs and top-level edges only for execution
prerequisites. Root and child scopes SHALL reject legacy endpoint bindings, data edges with target input ports, and
`control_edges`. The effective scope version SHALL still govern unrelated features such as Task-success prerequisites.

#### Scenario: Reject legacy connections

- **WHEN** a definition declares an old version or inherits it from the root
- **THEN** neither parsing nor serialization falls back to the old connection syntax

#### Scenario: Preserve startup and streaming support

- **WHEN** a workflow uses the new version with existing startup parameters or streaming execution
- **THEN** version predicates preserve those capabilities rather than treating them as unsupported

### Requirement: Declare input sources on the consumer

Each ordinary input and each `exclusive` candidate SHALL use the whole-output configuration reference
form `{"$ref":"node.port"}`. Exclusive lists SHALL contain at least two distinct
sources. Duplicate input keys, unknown fields, null bindings, mixed variants, and unsupported shapes MUST fail. Empty
maps SHALL be omitted on serialization.

#### Scenario: Bind an ordinary input

- **WHEN** `inputs.repository` is `{"$ref":"params.repository"}` in any supported scope
- **THEN** that output binds the consumer input and establishes its dependency without a separate edge

#### Scenario: Address dotted names before provider discovery

- **WHEN** a `$ref` input names an output with dots in either component
- **THEN** structural planning uses the unique in-scope node prefix followed by a nonempty port, including implicit scope sources
- **AND** unknown or ambiguous prefixes fail rather than silently changing endpoints on read or serialization

#### Scenario: Bind exclusive alternatives

- **WHEN** an input declares local and CI sources in an exclusive list
- **THEN** they form one existing statically proven candidate group without a merge node

#### Scenario: Reject duplicate input keys

- **WHEN** source JSON repeats an input-map key
- **THEN** parsing rejects it rather than losing the duplicate through an intermediate map

### Requirement: Normalize before dependency analysis

All definition-based compiler paths SHALL normalize authored input references before dependency analysis,
including supported bodies. Canonical data bindings SHALL be distinct from new authoring execution edges. Normalization
SHALL preserve source definitions, remain deterministic, and avoid duplicate dependencies. The graph description SHALL expose ordinary and exclusive input routes as data edges.

#### Scenario: Reject a combined cycle

- **WHEN** an input reference completes a cycle with an execution dependency
- **THEN** structural validation rejects it before execution even if a branch may skip

#### Scenario: Preserve source serialization

- **WHEN** callers plan the same definition repeatedly
- **THEN** its source version and serialized field meanings remain unchanged and canonical dependencies are not duplicated

#### Scenario: Normalize body inputs

- **WHEN** a supported body references a permitted local or synthetic source
- **THEN** normalization preserves the existing scope and invocation rules

### Requirement: Preserve data-binding contracts

Normalized references SHALL reuse existing connectivity, type, cycle, ancestry, scope, domain, exclusive-proof, and
input-resolution checks. Shared identity, sensitivity, false/null, per-frame selection, and typed optimization
eligibility or fallback SHALL remain unchanged. Missing values, failure, and cancellation MUST NOT select fallback
candidates. Isolated execution SHALL still accept resolved inputs without predecessors.

#### Scenario: Preserve conservative exclusive proof

- **WHEN** candidate mutual exclusion cannot be proven
- **THEN** compilation rejects the group even if a separate execution gate might skip

#### Scenario: Preserve shared secrets

- **WHEN** a selected reference forwards a shared object with secret fields
- **THEN** generated and in-memory execution preserve its identity and sensitivity

#### Scenario: Reject a cross-domain reference

- **WHEN** a reference violates existing input-domain rules
- **THEN** compilation rejects it without adding correlation or buffering

### Requirement: Share reference syntax without inferring execution from configuration

Input bindings SHALL select whole outputs without literals, projection, interpolation, or fallback expressions.
Configuration references SHALL retain existing explicit-ancestor and missing/skipped-read rules. Input and execution
dependencies MAY establish ancestry, but mentioning an output in configuration SHALL NOT create a dependency.

#### Scenario: Read through explicit ancestry

- **WHEN** an input binding or valid execution prerequisite establishes a context producer as an ancestor
- **THEN** existing context validation may accept the reference while retaining output-availability checks

#### Scenario: Reject undeclared context ancestry

- **WHEN** configuration mentions a producer without a dependency path
- **THEN** compilation retains its existing rejection

#### Scenario: Inspect input routing

- **WHEN** a runner previews a consumer with an ordinary input, an exclusive input, and a configuration reference
- **THEN** its description includes the ordinary data edge and one exclusive edge per candidate, but no edge for the configuration read
