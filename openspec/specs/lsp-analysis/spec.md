# lsp-analysis Specification

## Purpose

Provide language-neutral semantic evidence through an input-driven Stream node with bounded language-server sessions and one terminal result per analysis job.

## Requirements

### Requirement: Execute as a stream producer

The LSP analysis provider SHALL use the existing input-driven Stream execution contract. Waiting for server I/O SHALL NOT occupy the event coordinator. Invocations SHALL follow ordinary producer serialization, bounded emission admission, cancellation, and downstream-drain rules without runtime-specific LSP hooks.

#### Scenario: Analyze while other work is ready

- **WHEN** a language server is waiting to return a response
- **THEN** independent stream work and event timers can progress without waiting for an LSP callback to return

#### Scenario: Close upstream after admitting a job

- **WHEN** the upstream source closes after delivering an analysis batch
- **THEN** the admitted batch can finish and its terminal result drains before normal workflow completion

### Requirement: Preserve opaque caller context

Each input SHALL contain an analysis request and an opaque context value. Each terminal output SHALL include the same context value with its runtime value metadata preserved. The LSP provider SHALL NOT interpret this value as a test plan, log it as protocol data, or require a direct downstream dependency on its original input domain.

#### Scenario: Forward a revision comparison

- **WHEN** a caller supplies its original changes and snapshot identities inside context
- **THEN** the analysis output forwards them unchanged without understanding their schema

#### Scenario: Keep jobs isolated

- **WHEN** two inputs carry different context values
- **THEN** each terminal result carries only its matching input context and evidence

### Requirement: Publish one terminal outcome

Each analyzed, incomplete, or unrequested batch SHALL emit exactly one terminal result with status `analyzed`, `incomplete`, or `not_requested`. Progress notifications SHALL NOT become business emissions. Analysis and required session cleanup SHALL finish before publication so downstream consumers cannot act on provisional evidence.

#### Scenario: Receive progress before the final response

- **WHEN** the server emits several progress notifications and partial responses
- **THEN** the provider exposes progress through observations and emits only one aggregated terminal result

#### Scenario: Finish cleanup before downstream planning

- **WHEN** an analysis batch has produced all requested evidence
- **THEN** the provider closes its sessions before admitting the terminal result and performs no further fallible analysis after admission

### Requirement: Pass through unrequested analysis

An empty analysis request batch SHALL produce one `not_requested` result with the original context and no invented semantic evidence. It SHALL NOT launch or initialize a language server. This path SHALL remain an ordinary output message rather than a skipped node or an absent emission.

#### Scenario: Receive no semantic targets

- **WHEN** the configured analysis batch contains no target source ranges
- **THEN** the provider starts no server and the downstream consumer still receives the original comparison context

### Requirement: Bind evidence to snapshots

Each result SHALL identify its source snapshot, root, server configuration, and compilation configuration. Base and head SHALL use isolated sessions. Symbol locations SHALL be snapshot-local, and evidence from another snapshot or configuration SHALL NOT satisfy the batch.

#### Scenario: Analyze both revisions

- **WHEN** the same source path contains different definitions in base and head
- **THEN** results retain the separate snapshot identities and never resolve one revision against the other's session

### Requirement: Negotiate standard semantic queries

The provider SHALL negotiate statically advertised capabilities and document position encoding before batched symbol, reference, and call-hierarchy queries. It SHALL NOT advertise dynamic capability registration. Unsupported operations SHALL produce explicit gaps rather than successful empty results. Standard query handling SHALL NOT require Cargo, test identities, or Rust-specific methods.

#### Scenario: Use a standard-only server

- **WHEN** a server supports symbols and references but not call hierarchy or Rust extensions
- **THEN** the provider returns supported evidence and an explicit unsupported-capability gap without sending unconfigured extension methods

#### Scenario: Locate a symbol after non-ASCII text

- **WHEN** source bytes and negotiated LSP offsets differ
- **THEN** the provider converts locations against the exact document content

### Requirement: Attribute readiness profiles and uncertainty

Server-specific readiness SHALL be supplied by explicit profiles and retain its provenance. Semantic queries SHALL use the standard symbol, reference, and call-hierarchy methods; a Rust readiness profile SHALL NOT implicitly enable test-specific extension queries. Successful indexing, empty relationships, and status `analyzed` SHALL NOT assert complete test-impact knowledge. Partial evidence and the reasons for incompleteness SHALL remain distinguishable.

#### Scenario: Find no relationships

- **WHEN** standard semantic queries return no relationships
- **THEN** the result records the observation without claiming that tests outside the result are independent

### Requirement: Bound analysis and clean up owned processes

Analysis SHALL have an overall deadline and finite request/result limits. A missing server, timeout, server failure, or exhausted limit SHALL produce an `incomplete` result with a distinguishable reason after cleanup. Invalid requests and unrecoverable runtime or cleanup failures SHALL fail execution without publishing a success-shaped result.

#### Scenario: Exhaust the analysis budget

- **WHEN** a response does not arrive before the batch deadline
- **THEN** the provider terminates and reaps owned server processes and emits one incomplete result identifying the timeout

#### Scenario: Reject a malformed request

- **WHEN** the input request violates the node contract
- **THEN** the provider fails rather than emitting analyzed evidence

### Requirement: Cancel blocked I/O without fallback publication

Cancellation or instance disposal SHALL interrupt owned pipe reads and process waits, terminate and reap server children, and stop publication. An emitter failure SHALL propagate and SHALL NOT cause a replacement result. The provider SHALL NOT rely solely on the emitter to observe cancellation while waiting for the server.

#### Scenario: Cancel an idle server

- **WHEN** the server has produced no output and the Flow is cancelled
- **THEN** the invocation stops within the bounded shutdown period without emitting a fallback result or leaving a child process running

#### Scenario: Fail while waiting for output admission

- **WHEN** downstream failure interrupts a blocked terminal send
- **THEN** the provider returns the emitter failure and does not emit another result

### Requirement: Reuse sessions only within a batch

All queries for one snapshot/configuration within an invocation SHALL reuse one session. Owned sessions SHALL be closed on success, failure, or cancellation. Flow values SHALL contain serializable evidence and context rather than live process handles; cross-invocation pooling and persistent semantic caches are not required.

#### Scenario: Query multiple changed files

- **WHEN** a batch requests relationships from several files in one snapshot
- **THEN** one initialized session serves those requests and is closed before the batch result is published

### Requirement: Keep preparation free of analysis side effects

Node preparation and manifest inspection SHALL expose stable contracts without launching a language server or inspecting the requested repository. Execution SHALL use the explicitly configured server and SHALL NOT download or install one automatically.

#### Scenario: Inspect without an installed server

- **WHEN** a developer compiles or inspects the Flow while the configured server is unavailable
- **THEN** the provider exposes its interface without launching the server
