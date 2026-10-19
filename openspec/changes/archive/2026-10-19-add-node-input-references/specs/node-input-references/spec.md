## ADDED Requirements

### Requirement: Select binding syntax by definition version

Version `2026-10-19` SHALL support node-local inputs and execution-only edges. Existing versions SHALL retain their
original data edges and control gates and reject node-local inputs. Bodies SHALL inherit the root version. Every JSON
and programmatic entry point MUST enforce version-specific fields without shape-based guessing. The new version SHALL
inherit previously supported features.

#### Scenario: Reject mixed formats

- **WHEN** a new-version document includes `control_edges` or an edge with `to_input`, or a legacy document includes node-local inputs
- **THEN** validation rejects the version-incompatible field, including within bodies

#### Scenario: Preserve startup and streaming support

- **WHEN** a workflow uses the new version with existing startup parameters or streaming execution
- **THEN** version predicates preserve those capabilities rather than treating them as unsupported

### Requirement: Declare input sources on the consumer

New-version nodes SHALL accept an optional input-port map whose values are strict `from` endpoint objects or `exclusive`
endpoint lists. Endpoints SHALL name node and port separately. Exclusive lists SHALL contain at least two distinct
sources. Duplicate input keys, unknown fields, null bindings, mixed variants, and unsupported shapes MUST fail. Empty
maps SHALL be omitted on serialization.

#### Scenario: Bind an ordinary input

- **WHEN** `inputs.repository.from` names node `params` and port `repository`
- **THEN** that output binds the consumer input and establishes its dependency without a separate edge

#### Scenario: Bind exclusive alternatives

- **WHEN** an input declares local and CI sources in an exclusive list
- **THEN** they form one existing statically proven candidate group without a merge node

#### Scenario: Reject duplicate input keys

- **WHEN** source JSON repeats an input-map key
- **THEN** parsing rejects it rather than losing the duplicate through an intermediate map

### Requirement: Normalize before dependency analysis

All definition-based compiler paths SHALL normalize version-selected author definitions before dependency analysis,
including supported bodies. Canonical data bindings SHALL be distinct from new authoring execution edges. Normalization
SHALL preserve source definitions, remain deterministic, and avoid duplicate dependencies. Equivalent legacy and new
workflows without success edges SHALL have equivalent semantic graphs.

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

### Requirement: Keep configuration references separate

Input bindings SHALL select whole outputs without literals, projection, interpolation, or fallback expressions.
Configuration references SHALL retain existing explicit-ancestor and missing/skipped-read rules. Input and execution
dependencies MAY establish ancestry, but mentioning an output in configuration SHALL NOT create a dependency.

#### Scenario: Read through explicit ancestry

- **WHEN** an input binding or valid execution prerequisite establishes a context producer as an ancestor
- **THEN** existing context validation may accept the reference while retaining output-availability checks

#### Scenario: Reject undeclared context ancestry

- **WHEN** configuration mentions a producer without a dependency path
- **THEN** compilation retains its existing rejection

### Requirement: Preserve legacy definitions during migration

Existing versions SHALL retain their original serialized shapes and behavior. Migration to the new version SHALL move
data mappings into inputs and output gates into edges without changing their semantics. Branch-output gates MUST NOT be
replaced automatically by node-success edges. Updated readers SHALL retain legacy support; unsupported versions MUST
fail explicitly.

#### Scenario: Migrate conditional convergence

- **WHEN** an old exclusive group is moved into inputs and its control gates are moved into new edges
- **THEN** its data selection, branch activation, results, and existing proof remain equivalent

#### Scenario: Run an unchanged legacy workflow

- **WHEN** a caller loads a supported old definition
- **THEN** its edges retain data-binding semantics and no format rewrite is required
