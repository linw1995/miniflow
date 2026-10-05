# workflow-runtime-execution Specification

## Purpose

Prepare one executable Flow and schedule its synchronous execution domains through one runtime in oneshot and stream modes.

## ADDED Requirements

### Requirement: Prepare one executable Flow for both modes

The compiler and in-memory APIs SHALL prepare both modes into one Flow containing mode, prepared executors, execution domains, message-domain metadata, dependencies, outputs, and limits. One runtime SHALL execute both modes in memory and generated runners. Missing stream configuration selects oneshot; mode MUST NOT be inferred from executor kind. Preparation MUST reject unsupported node kinds before invocation.

#### Scenario: Prepare a oneshot workflow

- **WHEN** a valid task-only workflow has no stream execution configuration
- **THEN** preparation produces the common Flow in oneshot mode and the runtime executes it once

#### Scenario: Prepare a stream workflow

- **WHEN** a valid workflow declares stream execution and contains supported task, event, or stream nodes
- **THEN** preparation produces the same Flow type in stream mode and the runtime starts a stream instance

#### Scenario: Reject a node kind that does not support the selected mode

- **WHEN** a oneshot workflow contains an event or stream executor
- **THEN** preparation reports the node ID and mode before invoking any executor

#### Scenario: Use the common runtime in generated runners

- **WHEN** generated oneshot and stream runners execute their embedded plans
- **THEN** both call the same runtime and domain scheduler

### Requirement: Partition task graphs into synchronous execution domains

The planner SHALL assign every ordinary task node to exactly one execution domain. Each domain SHALL be a maximal serial region in validated topological order, split at forks, joins, and event or stream boundaries. Fork branches SHALL become separate dependent domains; a fan-in domain SHALL depend on every incoming branch. The domain DAG MUST preserve all data and control dependencies.

#### Scenario: Keep a linear chain in one domain

- **WHEN** a workflow is a simple task chain without forks or joins
- **THEN** its tasks belong to one domain and execute synchronously in topological order

#### Scenario: Split a fan-out into branch domains

- **WHEN** one task feeds two independent task branches
- **THEN** the upstream domain ends at the fork and each branch is assigned to a separately schedulable domain

#### Scenario: Make a join depend on every branch

- **WHEN** two branch domains feed a join task
- **THEN** the join's domain depends on both branches and cannot start until both complete

### Requirement: Keep execution domains distinct from message domains

An execution domain SHALL define a serial scheduling unit. A message domain SHALL define stream message identity and ordering. A oneshot invocation SHALL have one message identity and MAY contain multiple execution domains. Stream sources and event or producer emissions SHALL retain their existing message-domain boundaries. Splitting an execution domain MUST NOT create, merge, or broadcast message identities.

#### Scenario: Preserve one-shot message identity across branches

- **WHEN** a oneshot fan-out creates separate execution domains for two branches
- **THEN** both branch results belong to the same invocation and can rejoin in one fan-in domain

#### Scenario: Preserve stream message boundaries

- **WHEN** an event or stream node emits a new message
- **THEN** its existing message-domain boundary is preserved independently of execution-domain partitioning

### Requirement: Schedule ready execution domains concurrently

The runtime SHALL execute tasks in each domain synchronously in validated topological order. It MAY run independent ready domains concurrently, choosing them by stable topological position and node-ID tie-break. A domain becomes ready only after all predecessor-domain results are committed. A limit of one SHALL execute domains serially in topological order.

#### Scenario: Run independent branches concurrently

- **WHEN** two branch domains are ready and the worker limit is at least two
- **THEN** the runtime can execute both domains at the same time

#### Scenario: Wait at a domain join

- **WHEN** a domain has multiple predecessor domains
- **THEN** it starts only after every predecessor has committed its results

#### Scenario: Preserve serial execution with one worker

- **WHEN** the effective worker limit is one
- **THEN** ready domains execute serially in validated topological order

### Requirement: Preserve ordered scope effects

When a Flow executes inside a Loop or Iteration scope, the runtime SHALL schedule its execution domains serially in validated topological order. This preserves ordered Loop writes and lets a Loop exit prevent later body domains from running.

#### Scenario: Stop later body domains after a Loop exit

- **WHEN** a Loop body contains independent ready domains and one domain requests exit
- **THEN** the runtime completes the exit domain before scheduling later body domains and skips domains after the exit position

#### Scenario: Preserve iteration item parallelism

- **WHEN** an Iteration node runs independent item bodies in parallel
- **THEN** each item's body domains execute serially while item results retain input order

### Requirement: Bound and configure domain concurrency

The runtime SHALL cap active execution domains per workflow instance at a finite limit, defaulting to four workers. Callers MAY override it. In stream mode, existing `execution.limits.workers` remains an upper bound; the effective limit is the lower of runtime and stream limits. Domain queues and frame retention MUST remain bounded.

#### Scenario: Respect the runtime worker limit

- **WHEN** more domains are ready than the configured limit
- **THEN** no more than that limit execute concurrently and remaining work stays bounded

#### Scenario: Apply stream and runtime limits together

- **WHEN** stream execution configures fewer workers than the caller's runtime limit
- **THEN** active domains do not exceed the stream worker limit

### Requirement: Isolate execution-domain state and commit results

Each domain invocation SHALL receive a private context seeded with committed predecessor values and visible scope state. Nodes within the domain MAY share that context serially; concurrent domains MUST NOT share mutable context. The runtime SHALL validate and commit domain outputs and staged effects once before scheduling dependent domains.

#### Scenario: Keep concurrent branch contexts isolated

- **WHEN** one branch domain completes while another is still executing
- **THEN** the running domain cannot observe the completed result unless the dependency graph makes it an ancestor

#### Scenario: Commit a scope effect before dependent work

- **WHEN** a domain stages a Loop scope write and its dependent domain becomes ready
- **THEN** the write is committed exactly once before the dependent domain starts

#### Scenario: Reject an invalid domain result

- **WHEN** a task returns an output that violates its prepared port contract
- **THEN** the runtime does not publish that output or schedule dependent domains and attributes the failure to its node

### Requirement: Preserve node-kind state ownership

Ordinary tasks SHALL execute serially within their domain. An EventNode instance SHALL receive callbacks serially. A stateful StreamNode SHALL serialize invocations of its own instance while independent domains and producers progress. Producer backpressure MUST NOT occupy all execution capacity required by downstream domains.

#### Scenario: Serialize one event node's callbacks

- **WHEN** input and timer events become ready for the same event node
- **THEN** its callbacks run in one defined serial order while unrelated domains can progress

#### Scenario: Serialize one producer's invocations

- **WHEN** multiple messages are ready for the same stateful stream producer
- **THEN** one invocation finishes before the next begins on that producer instance

#### Scenario: Keep downstream progress under producer backpressure

- **WHEN** a producer blocks while its output queue is full
- **THEN** the runtime retains capacity for downstream domains that can release the queue

### Requirement: Stop scheduling and drain started domains on failure

When a node fails, the runtime SHALL stop admitting new workflow work and SHALL NOT start unstarted domains. It SHALL let already-started domains and synchronous calls settle, suppress results that are no longer publishable, and return a node-attributed failure. Stream mode SHALL retain already delivered outputs and suppress pending and later publications.

#### Scenario: Fail one parallel branch

- **WHEN** a task in one domain fails while another domain is still executing
- **THEN** the runtime starts no new domains, waits for started work to settle, and reports the failing node

#### Scenario: Suppress a late domain result after failure

- **WHEN** an already-started domain returns after another domain has failed
- **THEN** its result does not schedule dependents or publish a workflow output

### Requirement: Preserve deterministic workflow output order

Concurrent domain completion SHALL NOT change declared output names, port selection, or output order. Oneshot returns selected outputs after required domains settle. Stream outputs SHALL retain message-domain order and delivery acknowledgment semantics.

#### Scenario: Complete selected outputs out of order

- **WHEN** independent selected-output domains complete out of dispatch order
- **THEN** oneshot results follow declared output order and stream results follow their message-domain sequence
