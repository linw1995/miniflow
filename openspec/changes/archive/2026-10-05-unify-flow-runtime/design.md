# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. `Flow::prepare` validates a shared graph, but `into_tasks` converts it to a task-only Flow that runs one global topological list serially. Stream execution converts the nodes into `PreparedStream`/`StreamPlan` and uses a coordinator with `MessageDomains`; each message domain currently owns an ordered step list. Thus oneshot and stream both have ordered task work, but only stream has independent message domains and explicit bounded scheduling.

## Goals / Non-Goals

**Goals:**

- Store one validated executable Flow and execute it through one runtime in oneshot and stream modes.
- Partition the task graph into ordered synchronous execution domains and concurrently schedule independent ready domains.
- Keep execution-domain scheduling separate from stream message identity and ordering.
- Preserve stream timers, producer backpressure, per-message-domain FIFO, drain, and failure behavior.
- Apply domain scheduling to nested Loop bodies without allowing nested work to deadlock the shared worker limit.

**Non-Goals:**

- Inferring stream mode from the presence of an event or stream node.
- Running tasks concurrently within one execution domain, EventNode callbacks concurrently on one instance, or overlapping calls to one stateful StreamNode instance.
- Distributed execution, retries, rollback of plugin side effects, or changing data/control dependency semantics.

## Decisions

### 1. Store one executable Flow with two explicit domain maps

The common Flow plan stores execution mode, prepared executor variants, validated node/port bindings, outputs, and two related but distinct partitions:

- An **execution domain** is a serial scheduling region of the graph.
- A **message domain** identifies stream message ownership and FIFO order.

A oneshot invocation has one message identity and can contain several execution domains. In stream mode, source and event/producer emissions continue to create message domains; one message frame can pass through several execution domains without changing its identity. Splitting an execution domain never creates or merges a message domain.

The mode remains explicit: missing stream configuration selects oneshot, while stream configuration selects
stream mode. Preparation validates node-kind compatibility before any executor runs. A per-run `FlowRuntime`
owns the common plan and scheduler. Oneshot completion and stream admission/timer/drain behavior are lifecycle
policies of that runtime, not separate executable graph types. Existing `Flow::execute` and stream-instance
APIs can delegate as compatibility facades; `PreparedStream` and `StreamPlan` become internal lifecycle state
over the common plan.

Keeping independent task-only and stream execution plans was rejected because it duplicates graph state and scheduling behavior. Inferring stream mode from a node kind was rejected because mode also defines source ownership, resource limits, and lifecycle semantics.

### 2. Partition the graph into maximal serial execution domains

Use the validated data and control dependency graph to partition ordinary tasks. Count distinct predecessor
and successor nodes, not ports. Continue a serial domain across an edge only when its source has one successor,
its target has one predecessor, and neither endpoint is an event/stream boundary. This forms maximal linear
task regions. A fork ends the upstream domain; each branch starts a separately schedulable domain. A join
starts a domain that depends on all incoming branch domains. Independent roots form independent domains.
Event and Stream nodes remain explicit single-node boundaries and keep their mode-specific executor handling.
Every graph node belongs to one execution domain, and cross-domain edges preserve the original dependencies.

For example, a chain `A -> B -> C` is one serial domain. A diamond `A -> {B, C} -> D` becomes an upstream domain for `A`, independent branch domains for `B` and `C`, and a join domain for `D`. Domain order uses each domain's earliest node position in the stable topological order.

This boundary rule was chosen over one task per domain to avoid worker dispatch overhead on ordinary chains, and over topological waves because a slow branch should not delay ready descendants on a faster branch. It also makes the serial contract explicit and easy to test.

### 3. Schedule domains with one bounded runtime coordinator

The runtime maintains a dependency DAG of execution domains. It dispatches ready domains by stable topological priority up to the effective worker limit. Each worker runs the domain's tasks synchronously and in topological order, resolves intra-domain inputs from the domain-local context, validates each node result, and returns one domain completion. The coordinator commits the completion and makes dependent domains ready. A domain with multiple predecessors waits for all of them.

Both lifecycle policies consume the same `ExecutionDomains` plan and `RuntimeOptions` budget. Oneshot starts the graph's root domains for one invocation and completes after terminal domains and selected outputs settle. Stream mode schedules domains within each message frame while retaining FIFO admission within every `MessageDomain`; independent message domains remain independently progressable subject to capacity. A worker limit of one serializes all ready domains by stable priority.

When a Flow runs inside a Loop or Iteration scope, its effective domain limit is one. Scope writes and exit requests are ordered effects, so serial dispatch preserves their established topological semantics. Iteration still runs separate item bodies concurrently under the shared domain budget.

`RuntimeOptions.max_parallel_domains` defaults to four and can be overridden. Existing stream `execution.limits.workers` remains a cap; the effective limit is the lower of runtime and stream settings. Stream producers retain bounded dedicated/reserved execution lanes, and EventNode callbacks stay on their serial coordinator path. Those lanes belong to the same runtime; they are not merged into a queue where a producer blocked on output capacity could starve downstream domains.

### 4. Give each domain an isolated context and explicit completion record

At dispatch, the coordinator forks a domain-local context from committed predecessor outputs, workflow arguments, visible Loop scope, cancellation state, and the remaining step budget. Tasks within the same domain share that context serially, matching current behavior. Concurrent sibling domains receive isolated contexts. Values in one domain are not visible to another until the producing domain has completed and committed.

Each domain completion contains per-node validated results plus staged scope writes/exits and observation
records. The coordinator commits output ownership and context effects once. Loop writes from concurrent domains
are applied in stable domain/topological order at the Loop-pass boundary; a completed exit effect stops new
domain dispatch beyond that point, while already-started calls settle. Step budgets are reserved by the
coordinator before dispatch. Observation and snapshot records are merged by node identity and invocation path.
Scheduler locks are never held while plugin code runs.

Sharing one mutable context between workers was rejected because it races and serializes execution behind a lock. Replacing the whole context with each returned clone was rejected because sibling domains could overwrite one another's outputs or scope state.

### 5. Preserve executor ownership and streaming lifecycle rules

Ordinary tasks are synchronous steps inside a domain and can share that domain's private mutable context. A task
executor may be called from different domain instances only when the runtime contract permits it; the existing
`TaskNode: Send + Sync` bound remains required. One EventNode instance continues to receive input, timer, and
close callbacks serially. Calls to one StreamNode instance remain serial because its mutable state is `Send`
but not necessarily `Sync`; separate producers may progress concurrently.

The common runtime continues to own source admission, message-domain queues, timers, output acknowledgment, cancellation, and drain. Failure stops new domain dispatch, wakes blocked sends, waits for started calls, and suppresses late publications. Already delivered stream outputs and external side effects cannot be rolled back.

### 6. Migrate callers without changing workflow schemas

No workflow JSON schema change is needed: oneshot remains the default when stream execution is absent, and stream definitions keep their mode and existing limits. Runtime callers gain `max_parallel_domains`; generated runners use the same default and accept the same override in both modes. Existing typed helpers can delegate during migration and be deprecated only if their return types cannot remain source-compatible.

Implementation proceeds by introducing the common Flow plan and domain partitioner behind current entry points, then routing in-memory and generated execution through `FlowRuntime`. After domain parity and backpressure coverage passes, duplicate task-only and stream-plan conversions are removed. Rollback restores the prior runtime while leaving workflow definitions valid because the serialized schema does not change.

## Risks / Trade-offs

- Independent domains can complete and perform external side effects in a different order → document that only dependency ordering is guaranteed; use one worker when serial behavior is required.
- Incorrect partition boundaries could split context-dependent work or merge a fork into a serial chain → derive boundaries from validated data/control edges, retain context-reference ancestry checks, and test linear, fork, join, and control-flow graphs.
- Forked domain contexts could lose Loop writes, exits, snapshots, or observations → model each as an explicit completion effect and cover nested Loop behavior before switching callers.
- A producer waiting for queue capacity could starve downstream work → preserve capacity for downstream domains and verify with a full queue and one task worker.
- Concurrent failures can race → stop unstarted domain dispatch on the first accepted failure, drain started work, and choose a node-attributed error deterministically among that started set.
- Concurrent domains could reorder stream frames → retain per-message-domain FIFO and bound active domains and pending emissions independently of worker count.
- Runtime options and stream limits could be miscombined → define the effective limit as their minimum and include it in diagnostics.
