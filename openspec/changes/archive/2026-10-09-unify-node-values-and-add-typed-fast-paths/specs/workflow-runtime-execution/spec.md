# Spec Delta

## ADDED Requirements

### Requirement: Execute typed segments through the common domain lifecycle

Generated typed segments SHALL run through the existing prepared domain scheduler and per-node lifecycle. They SHALL
preserve dependency order, private contexts, worker limits, node attribution, observation phases, failure draining, and
transactional effects. Each invocation SHALL own independent typed values. Execution MUST NOT instantiate nodes,
reconstruct graphs, or repartition domains.

#### Scenario: Run parallel independent domains

- **WHEN** two ready domains contain generated typed segments
- **THEN** the existing worker limit and private-context isolation apply without sharing mutable typed frames

#### Scenario: Attribute a typed business failure

- **WHEN** an internal typed task returns a source-bearing business error
- **THEN** execution retains its original error chain and node observation phase, stops admitting new work, and drains
  started domains

#### Scenario: Repeat an invocation

- **WHEN** the same prepared generated workflow executes twice
- **THEN** neither invocation reuses moved values or mutable frame state from the other invocation

### Requirement: Preserve dependency presence independently of typed ownership

Typed transfer SHALL retain existing distinctions between present values, optional omission, explicit skips, and
unexpectedly missing outputs. Missing-dependency checks SHALL precede skip decisions. Skipped tasks MUST NOT assemble or
decode input structs or invoke business logic. Moving a payload MUST NOT erase presence evidence required by the
validated plan.

#### Scenario: Skip an internal typed successor

- **WHEN** a producer explicitly skips the output required by its next typed task
- **THEN** the successor is skipped without constructing an input or accessing a missing Rust field

#### Scenario: Preserve missing-before-skip precedence

- **WHEN** a target has an explicitly skipped dependency and another unexpectedly missing dependency
- **THEN** the missing dependency error is reported with its original cause before the target is skipped or invoked

#### Scenario: Preserve null payload identity

- **WHEN** an eligible required shared-value field contains JSON null
- **THEN** direct transfer retains its shared payload identity and treats it as present data

### Requirement: Validate typed results before any successor observes them

Generated execution SHALL enforce all produced fields' prepared contracts before making any result available, including
fields without consumers. Equivalent typed validation SHALL preserve strict representations, finite-float rules,
refinements, escaped pointers, typed sources, and producer attribution. Required dynamic encoding SHALL finish before
publication or any successor call that depends on its success.

#### Scenario: Reject an invalid unused descendant

- **WHEN** an internal producer returns a non-finite float in an unused nested collection
- **THEN** execution reports the producer and escaped nested path, invokes no dependent consumer, and publishes none of
  the result

#### Scenario: Reject an invalid prepared refinement

- **WHEN** a typed result is structurally valid for its Rust struct but violates its prepared output refinement
- **THEN** execution fails before any successor sees that result with the same typed mismatch provenance as dynamic
  execution

#### Scenario: Reject boundary encoding atomically

- **WHEN** encoding a typed segment's dynamic boundary result fails after another field encoded successfully
- **THEN** no partial result is committed and no dependent domain is scheduled

### Requirement: Preserve dynamic observation of generated execution

Typed generation SHALL preserve declared context reads, selected outputs, and node lifecycle observations. When payload
snapshots or other runtime observers require intermediate dynamic values unsupported by the typed segment, execution
SHALL select its prepared dynamic fallback without runtime graph planning. Observers MUST NOT receive missing values
solely because a payload was moved.

#### Scenario: Request invocation snapshots

- **WHEN** a generated workflow prepared with eligible typed segments runs with input/output snapshot capture
- **THEN** affected domains use their dynamic fallback and snapshots match ordinary dynamic execution

#### Scenario: Keep lifecycle telemetry enabled

- **WHEN** an eligible typed segment runs with node lifecycle telemetry but no payload snapshots
- **THEN** normal node start, success, skip, and failure observations remain attributed to each original node

#### Scenario: Observe a transitive predecessor

- **WHEN** a declared contextual reader accesses a transitive predecessor output
- **THEN** generated execution materializes or retains that output through the dynamic path and exposes the same value
  as in-memory execution
