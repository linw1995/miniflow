# Spec Delta

## ADDED Requirements

### Requirement: Select execution kind during preparation

A prepared node SHALL contain a task or event executor alongside its metadata. Event providers SHALL
construct event state directly and MUST NOT require a task execution method. Tasks SHALL retain
`Send + Sync`; event state MAY be `Send` without `Sync` and SHALL be invoked through exclusive mutable
access. The event contract SHALL accept input, timer, and upstream-close events and return complete
emissions and deadline updates.

#### Scenario: Construct event state directly

- **WHEN** a factory returns an event implementation containing Send-only mutable state
- **THEN** preparation succeeds without a task adapter or an additional state-construction hook

### Requirement: Restrict synchronous execution to tasks

Synchronous flows and generated task bodies SHALL contain only task executors. An event node MUST be
rejected during preparation of a synchronous flow, before any executor is invoked. Metadata inference
SHALL remain available independently of the execution kind.

#### Scenario: Reject an event in a synchronous flow

- **WHEN** a caller supplies a prepared event node to synchronous flow construction
- **THEN** construction fails with the definition identity and does not invoke the event
