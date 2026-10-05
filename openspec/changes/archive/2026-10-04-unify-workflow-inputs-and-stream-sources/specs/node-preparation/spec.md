# Spec Delta

## ADDED Requirements

### Requirement: Declare execution resources during preparation

Prepared node metadata SHALL identify runtime input resources required for execution, including exclusive
stdin and host-supplied channels. Preparation SHALL validate resource conflicts without acquiring those
inputs. Resource discovery MUST use the selected provider's metadata without built-in kind-name inference or a
second metadata-only factory contract.

#### Scenario: Prepare an external stdin provider

- **WHEN** a third-party stream provider declares exclusive runtime stdin
- **THEN** its resource requirements are available for validation and launch without reading stdin

#### Scenario: Reject conflicting ownership

- **WHEN** two prepared nodes declare exclusive use of the same input resource
- **THEN** preparation reports both consumers and the resource before execution

## MODIFIED Requirements

### Requirement: Select execution kind during preparation

A prepared node SHALL contain a task, event, or stream executor alongside its metadata. Event and stream
providers SHALL construct their state directly and MUST NOT require a task execution method. Tasks SHALL
retain `Send + Sync`; event and stream state MAY be `Send` without `Sync` and SHALL be invoked through
exclusive mutable access. The event contract SHALL accept input, timer, and upstream-close events and
return complete emissions and deadline updates. Stream executors SHALL emit results incrementally
during each startup or input invocation. An initial EventNode without upstream dependencies MUST be
rejected because the event contract does not define autonomous startup.

#### Scenario: Construct event state directly

- **WHEN** a factory returns an event implementation containing Send-only mutable state
- **THEN** preparation succeeds without a task adapter or an additional state-construction hook

#### Scenario: Construct producer state directly

- **WHEN** a factory returns a stream implementation containing Send-only mutable state
- **THEN** preparation exposes its metadata without invoking the producer

#### Scenario: Reject an autonomous event node

- **WHEN** an event executor is placed at the workflow root without any incoming dependency
- **THEN** preparation requires an explicit activation source and does not invent an initial input or timer
  event
