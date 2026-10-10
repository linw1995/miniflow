# Runtime future execution

## Purpose

Let synchronous node providers drive owned asynchronous operations through an optional shared execution facility without requiring every workflow to adopt an asynchronous runtime dependency or scheduling model.

## ADDED Requirements

### Requirement: Keep future execution optional

The runtime SHALL offer future execution through opt-in Cargo features. Its default dependency graph MUST NOT acquire an asynchronous executor from this facility. Public node and execution-context contracts MUST NOT expose backend-specific runtime types. Selecting a native HTTP transport MAY independently introduce its reactor dependency.

#### Scenario: Build an ordinary workflow

- **WHEN** a workflow selects only synchronous node packages and leaves future execution disabled
- **THEN** this facility adds no executor dependency or runtime initialization

#### Scenario: Build portable future support

- **WHEN** a consumer selects portable future execution without a concrete native reactor or HTTP backend
- **THEN** future-driving support builds without Tokio in the normal dependency graph

#### Scenario: Select a transport with a reactor

- **WHEN** a node uses portable future driving with a Tokio-based HTTP transport
- **THEN** the transport's Tokio dependency is reported separately from the optional workflow-managed backend

### Requirement: Preserve synchronous task execution

Future-based providers SHALL remain ordinary synchronous tasks with the same input decoding, context access, result validation, publication, and skip boundaries. The future driver SHALL block the invoking synchronous thread until success or failure. Existing concurrency limits SHALL continue to bound node invocations.

#### Scenario: Complete a future-based task

- **WHEN** a future returns a task's terminal value
- **THEN** ordinary output conversion and validation finish before dependent nodes can observe it

#### Scenario: Skip a future-based node

- **WHEN** upstream skip propagation skips the node
- **THEN** its future is never polled and its execution resources are not initialized

#### Scenario: Bound concurrent calls

- **WHEN** several independent future-based tasks become ready
- **THEN** existing workflow concurrency limits bound their execution without unbounded detached spawning

### Requirement: Select a backend without changing node contracts

Portable future execution and an opt-in managed native backend SHALL accept the same future-driving contract. Backend selection SHALL follow resolved Cargo features without new workflow schema fields. Incompatible nested executor entry MUST fail with a typed runtime error rather than panic.

#### Scenario: Select the managed backend

- **WHEN** the managed backend feature is enabled for the resolved runtime package
- **THEN** the same node implementation drives its future using that backend

#### Scenario: Call from an incompatible executor context

- **WHEN** synchronous future driving would enter an incompatible already-active executor
- **THEN** the runtime reports a typed error and documents the host blocking-thread entry point

### Requirement: Share managed backend ownership by execution scope

A workflow-managed backend SHALL initialize lazily in the root execution scope and be shared by forked domain, subgraph, iteration, and stream contexts. Initialization MUST NOT occur during node preparation. Scoped ownership SHALL remain alive until started jobs finish and SHALL release resources when the owning scope ends.

#### Scenario: Reuse one backend across parallel domains

- **WHEN** parallel domains first request a managed backend in the same scope
- **THEN** they share one initialized backend instead of creating one per node or invocation

#### Scenario: Drive futures in nested bodies

- **WHEN** Loop or Iteration bodies use future-based nodes
- **THEN** their contexts share the root scope's backend throughout their execution

#### Scenario: Prepare a workflow without future calls

- **WHEN** providers are prepared or inspected but no future is executed
- **THEN** no managed backend or associated worker thread is created

#### Scenario: End an execution scope

- **WHEN** a scope finishes or fails after starting future-based jobs
- **THEN** it waits for started jobs before releasing its managed backend

### Requirement: Retain ownership of pending operations

The bridge SHALL retain ownership of the future until completion or failure. Cancellation and deadlines SHALL stop local polling and drop the operation without permitting later result publication. Cancellation waiter storage MUST remain bounded under repeated polls. Transport-owned resources SHALL retain their separately documented lifetime.

#### Scenario: Cancel a pending future

- **WHEN** the execution scope is cancelled while a future is pending
- **THEN** the future is dropped and no detached continuation can publish its result

#### Scenario: Repeatedly poll cancellation

- **WHEN** a pending operation is polled repeatedly before cancellation
- **THEN** waiter registration does not grow without bound and cancellation still wakes waiting execution

#### Scenario: Use a transport fallback

- **WHEN** a portable executor calls a transport that owns a process-wide fallback reactor
- **THEN** the transport's reactor lifetime is not described as workflow-scoped ownership

### Requirement: Preserve typed execution infrastructure errors

Backend initialization and incompatible execution-context failures SHALL preserve typed sources and identify shared runtime mechanics. Provider protocol and credential failures SHALL remain owned by the provider package and MUST NOT become runtime-specific provider variants.

#### Scenario: Fail managed backend initialization

- **WHEN** the managed backend cannot allocate its execution resources
- **THEN** the node receives a typed runtime failure without a replacement backend or result publication

#### Scenario: Fail a provider request

- **WHEN** the driven future returns a provider protocol error
- **THEN** the provider retains that typed error and the runtime adds only shared execution context
