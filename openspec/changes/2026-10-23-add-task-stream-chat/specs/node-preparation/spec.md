## MODIFIED Requirements

### Requirement: Select execution kind during preparation

A prepared node SHALL contain a task, event, stream, or combined Task/Stream executor alongside its metadata. Combined
executors SHALL expose both output categories for one invocation. Event and pure stream providers SHALL construct their
state directly and MUST NOT require a task execution method. Tasks and combined executors SHALL retain `Send + Sync`;
event and pure stream state MAY be `Send` without `Sync` and SHALL be invoked through exclusive mutable access. The
event contract SHALL accept input, timer, and upstream-close events and return complete emissions and deadline updates.
Stream executors SHALL emit results incrementally during each startup or input invocation. An initial EventNode without
upstream dependencies MUST be rejected because the event contract does not define autonomous startup.

#### Scenario: Construct event state directly

- **WHEN** a factory returns an event implementation containing Send-only mutable state
- **THEN** preparation succeeds without a task adapter or an additional state-construction hook

#### Scenario: Construct producer state directly

- **WHEN** a factory returns a stream implementation containing Send-only mutable state
- **THEN** preparation exposes its metadata without invoking the producer

#### Scenario: Reject an autonomous event node

- **WHEN** an event executor is placed at the workflow root without any incoming dependency
- **THEN** preparation requires an explicit activation source and does not invent an initial input or timer event

#### Scenario: Prepare coexisting capabilities

- **WHEN** a provider constructs a combined executor
- **THEN** preparation reflects terminal and stream contracts without selecting one capability or invoking business logic

### Requirement: Restrict synchronous execution to tasks

Synchronous flows and generated task bodies SHALL contain only ordinary or combined Task executors. Pure event and stream nodes MUST be rejected before invocation. Combined stream exports SHALL be supported, while graph-local stream edges in synchronous bodies remain unsupported. Metadata inference SHALL remain independent of execution kind.

#### Scenario: Reject an event in a synchronous flow

- **WHEN** a caller supplies a prepared event node to synchronous flow construction
- **THEN** construction fails with the definition identity and does not invoke the event

#### Scenario: Reject a producer in a synchronous scope

- **WHEN** a pure stream node appears in a synchronous flow, Loop body, or Iteration body
- **THEN** preparation fails with its definition identity before executing the producer

#### Scenario: Export a combined node from a loop body

- **WHEN** a synchronous Loop body contains a combined node with an explicitly selected live export
- **THEN** preparation accepts the Task path and export while preserving synchronous body dependencies

## ADDED Requirements

### Requirement: Reflect typed combined contracts

Typed combined preparation SHALL derive inputs, terminal outputs, and stream outputs from their associated value contracts. Competing declarations and terminal/stream name collisions MUST fail before execution. Configuration-dependent providers SHALL retain a dynamic reflecting interface with the same validation rules.

#### Scenario: Preserve one source of truth

- **WHEN** typed preparation receives manually supplied stream ports competing with its associated emission contract
- **THEN** construction fails before returning an executor

#### Scenario: Reflect a dynamic combined provider

- **WHEN** a configuration-dependent provider declares distinct terminal and stream ports
- **THEN** type inference resolves both contracts without executing the provider
