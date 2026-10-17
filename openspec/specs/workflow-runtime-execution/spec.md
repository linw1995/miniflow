# workflow-runtime-execution Specification

## Purpose

Prepare one executable Flow and schedule its synchronous execution domains through one runtime in oneshot and stream modes.

## Requirements

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

### Requirement: Execute only complete validated plans

FlowRuntime SHALL accept only prepared Flows, stream plans, or Task targets from those plans. Preparation MUST validate executor kinds, dependencies, graph order, selected outputs, domain partitions, and message ownership before returning plans. Targets MUST retain original resolved contracts and validate isolated bindings without weakening graph validation. Execution MUST NOT instantiate nodes, validate graph structure, or construct domain plans.

#### Scenario: Build a generated oneshot workflow

- **WHEN** a generated workflow is prepared successfully
- **THEN** its execution APIs accept the prepared Flow and do not require a node registry

#### Scenario: Reject invalid construction before execution

- **WHEN** node construction or graph validation fails
- **THEN** preparation returns a construction error and no executor is invoked

#### Scenario: Prepare nested bodies

- **WHEN** a workflow contains Loop or Iteration bodies
- **THEN** their executable plans are constructed before the outer preparation succeeds

#### Scenario: Reject unresolved port references

- **WHEN** a constructor receives a data connection, control dependency, or output selection referencing an undeclared port
- **THEN** construction fails before execution, including direct Flow and prepared-stream construction APIs

#### Scenario: Execute a prepared isolated target

- **WHEN** a caller supplies a prepared Task target with validated isolated input and context bindings
- **THEN** execution uses its original resolved contracts without requiring a registry or running external dependencies

### Requirement: Execute compiled graph layouts in generated runners

Generated runners SHALL bind executors to immutable compiled dependency, node-index, output, execution-domain, and message-ownership records. Launch MUST NOT rebuild graphs, resolve edge names into indices, repartition domains, or derive ownership. Isolated selection SHALL map exact IDs to compiled indices without constructing a graph. In-memory and generated execution SHALL share plan records and runtime scheduling; isolated Tasks SHALL share the per-node lifecycle.

#### Scenario: Launch a generated fork and join

- **WHEN** a generated oneshot runner starts
- **THEN** its branch domains and join dependencies are loaded from constant plan data without graph construction

#### Scenario: Bind nested task bodies

- **WHEN** a generated Loop or Iteration is initialized
- **THEN** its body binds executors to the compiled body plan without repartitioning the graph

#### Scenario: Preserve dynamic execution state

- **WHEN** the same compiled graph is instantiated twice
- **THEN** immutable layout data is shared while contexts and mutable stream state remain independent

#### Scenario: Select a compiled middle task

- **WHEN** a runner selects a noninitial Task by its exact definition ID
- **THEN** dispatch uses its existing compiled index and resolved contract without removing edges or repartitioning domains

### Requirement: Preserve typed runtime lookup causes

Task dependency and workflow output lookup failures SHALL retain their original `NodeExecutionError` source. Dependency failures SHALL identify the consuming node and input or control dependency and retain the dependency observation phase. Selected-output failures SHALL identify the source node and declared workflow output name and retain the workflow output selection phase. Diagnostic formatting MUST NOT replace the source with a string.

#### Scenario: Preserve a missing data dependency

- **WHEN** a task requires an output that was not published
- **THEN** execution fails before invocation, identifies the consuming node and input, and retains the original lookup error source

#### Scenario: Preserve missing control dependency precedence

- **WHEN** a task has a skipped input and a missing control dependency
- **THEN** the missing dependency failure takes precedence over skipping, identifies the control dependency, and retains the original lookup error source

#### Scenario: Preserve a missing selected output

- **WHEN** a required or optional workflow output selects an output that was not published
- **THEN** selection fails with the declared output name and source node, retains the original lookup error source, and is attributed to workflow output selection

### Requirement: Retain snapshot sink failure provenance

The snapshot recorder SHALL accept source-bearing sink errors and retain the original cause until its owner presents diagnostics. After a recording failure, finishing the recorder MUST NOT erase that cause.

#### Scenario: Finish after a sink failure

- **WHEN** a snapshot sink fails with an I/O error during recording and the recorder is subsequently finished
- **THEN** its diagnostic retains the typed I/O cause and error kind

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

### Requirement: Reject isolated target contract drift

Generated isolated invocation SHALL compare its prepared target with the compiled target contract before
business execution. Executor category, port identities, resolved types, required flags, declared external
references, and stdin conditions MUST agree. Drift MUST fail with affected node, port, or resource context
and preserve the underlying typed error causes where present.

#### Scenario: Reject changed output metadata

- **WHEN** a prepared target exposes a different output type from its build-emitted target contract
- **THEN** selection fails before invoking the target with node and output context

#### Scenario: Reject changed context or resource requirements

- **WHEN** the prepared target changes a declared external reference or stdin condition
- **THEN** selection fails before reading input resources or invoking business logic

#### Scenario: Reword a diagnostic reference label

- **WHEN** a target's prepared references retain their IDs and types but change diagnostic labels
- **THEN** binding accepts the unchanged reference contract without treating label wording as interface drift

### Requirement: Preserve secret runtime value metadata

Runtime values SHALL support an immutable secret marker without changing their
JSON type, business accessors, equality, validation, or serialization. Marking a
container SHALL mark all descendants. Cloning, routing, shared typed codecs, and
field/index/pointer extraction MUST retain markers. Existing unmarked aliases
SHALL remain unchanged.

#### Scenario: Extract a field from a secret object

- **WHEN** a caller marks an object secret and extracts a nested field
- **THEN** both the object and extracted value remain secret and business reads return their original data
- **AND** aliases retained before marking remain unmarked

#### Scenario: Route sensitive values through tasks and streams

- **WHEN** marked values pass through forwarding tasks or accumulate in a Batch output
- **THEN** runtime results preserve their data and markers, including secret elements inside public containers

### Requirement: Propagate sensitivity into derived values explicitly

The runtime SHALL expose a way to mark a computed value when any supplied source
contains a secret. Public containers SHALL report sensitivity in their descendants.
The runtime MUST NOT infer that every output of a node is secret merely because
one input is secret. Ordinary owned decoding and JSON round trips SHALL NOT claim
to retain sensitivity metadata.

#### Scenario: Compute a credential header

- **WHEN** a caller computes a new string and inherits sensitivity from a secret token or a container holding it
- **THEN** the complete computed string becomes secret while its business value remains available

### Requirement: Retain metadata-only scope updates

Execution-domain scope merging MUST retain changes to secret metadata even when
the business JSON values compare equal.

#### Scenario: Mark an unchanged Loop variable secret

- **WHEN** a domain publishes a Loop variable with the same payload and a new secret marker
- **THEN** the enclosing scope and its published source output retain the marker

### Requirement: Share node input resolution across execution paths

Task, generated-task, and event execution SHALL share input assembly and validation.
Startup values and data dependencies SHALL populate ports; control dependencies SHALL
only determine readiness. Routing MUST preserve shared payloads and secret metadata.
Missing dependencies SHALL take precedence over skips. Skipped nodes SHALL bypass input
validation and invocation. Validated typed transfers SHALL not require dynamic payload lookup.

#### Scenario: Match task and event failure behavior

- **WHEN** task and event execution resolve the same inputs with a skipped dependency and a missing dependency
- **THEN** both report the missing dependency with its typed source before any node invocation

#### Scenario: Validate only runnable nodes

- **WHEN** supplied input values have an invalid type but a dependency is explicitly skipped
- **THEN** the node is skipped without type validation, unless a dependency is also missing

#### Scenario: Preserve partial failure snapshots

- **WHEN** task input resolution fails after collecting some input values
- **THEN** the failure snapshot retains those values with existing secret redaction and node attribution

#### Scenario: Preserve direct typed transfers

- **WHEN** a generated task receives an input through a typed slot with validated presence
- **THEN** shared input resolution does not require a corresponding dynamic value or duplicate the payload

### Requirement: Expose the executing node independently of observation

Task execution contexts SHALL expose the current definition-facing node ID without requiring telemetry.
Nested calls SHALL restore their parent identity after success, error, or unwinding, and forked execution
domains SHALL not inherit an actively executing node. Existing scope paths SHALL identify enclosing loop
invocations. This metadata MUST NOT install an implicit application usage collector.

#### Scenario: Report a generated node invocation

- **WHEN** a generated task requests its executing identity
- **THEN** it receives the same node ID and scope path as in-memory execution

#### Scenario: Restore parent identity after nested failure

- **WHEN** a nested task fails or panics
- **THEN** its parent context retains the parent node identity and the outer caller is restored afterward
