## ADDED Requirements

### Requirement: Explicitly export finite child streams

A Loop MAY declare stream outputs mapped to local TaskStream ports. Export names MUST be unique,
nonblank, and distinct from variable names. Definitions using exports SHALL require version 2026-10-28.

#### Scenario: Forward an incremental answer

- **WHEN** a connected child emits a declared output during a Loop pass
- **THEN** the containing Loop publishes the mapped value before the pass completes
- **AND** its final variables remain a separate terminal result

#### Scenario: Compose nested Loops

- **WHEN** a Loop exports a nested Loop's stream port
- **THEN** forwarding preserves the value and each scope's execution identity

### Requirement: Reuse normal execution and delivery rules

Forwarded children SHALL retain input guards, type validation, snapshots, exit cutoffs, cancellation,
and delivery errors. Forwarding SHALL propagate existing backpressure without an additional queue.
Disconnected execution SHALL retain ordinary terminal behavior.

#### Scenario: Skip an inactive prerequisite

- **WHEN** a child's success prerequisite is inactive
- **THEN** the child does not execute or emit through the containing Loop

#### Scenario: Fail delivery during a pass

- **WHEN** the consumer cancels or rejects an emission
- **THEN** execution propagates the typed failure and does not start later passes

### Requirement: Preserve prepared contracts and completion facts

Generated runners SHALL freeze exported source ports and reject drift before business work. A Loop
SHALL publish one terminal completion with its pass summary independently of its emission count.

#### Scenario: Reject a changed source stream port

- **WHEN** a generated runner's provider changes an exported port contract
- **THEN** startup fails before executing the child

#### Scenario: Complete after multiple emissions

- **WHEN** several passes emit values and finish successfully
- **THEN** observation retains their child scopes and one parent completion with the final pass count
