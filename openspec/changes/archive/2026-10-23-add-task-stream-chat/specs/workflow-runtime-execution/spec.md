## MODIFIED Requirements

### Requirement: Prepare one executable Flow for both modes

Both modes SHALL prepare one Flow with mode, executors, execution and message domains, dependencies, outputs, and limits. In-memory and generated runners SHALL use one runtime. Missing stream configuration selects oneshot; executor kind MUST NOT select the mode. Combined nodes SHALL work in both modes and expose host live exports in oneshot. Preparation MUST reject unsupported executors before invocation.

#### Scenario: Prepare a oneshot workflow

- **WHEN** a valid task-only workflow has no stream execution configuration
- **THEN** preparation produces the common Flow in oneshot mode and the runtime executes it once

#### Scenario: Prepare a stream workflow

- **WHEN** a valid workflow declares stream execution and contains supported task, event, stream, or combined nodes
- **THEN** preparation produces the same Flow type in stream mode and the runtime starts a stream instance

#### Scenario: Reject a node kind that does not support the selected mode

- **WHEN** a oneshot workflow contains a pure event or stream executor
- **THEN** preparation reports the node ID and mode before invoking any executor

#### Scenario: Use the common runtime in generated runners

- **WHEN** generated oneshot and stream runners execute their embedded plans
- **THEN** both call the same runtime and domain scheduler

#### Scenario: Keep a live chat workflow finite

- **WHEN** a oneshot workflow contains a combined node and a live export
- **THEN** the workflow remains finite and returns its complete selected Task outputs once

### Requirement: Partition task graphs into synchronous execution domains

Every ordinary Task SHALL belong to one maximal serial execution domain in validated topological order, split at forks, joins, and Event/Stream boundaries. Combined nodes SHALL split where graph-local consumers need independent progress. Fork branches SHALL become separate dependent domains; fan-in SHALL depend on all incoming branches. The domain DAG MUST preserve data and control dependencies.

#### Scenario: Keep a linear chain in one domain

- **WHEN** a workflow is a simple task chain without forks, joins, or stream boundaries
- **THEN** its tasks belong to one domain and execute synchronously in topological order

#### Scenario: Split a fan-out into branch domains

- **WHEN** one task feeds two independent task branches
- **THEN** the upstream domain ends at the fork and each branch is assigned to a separately schedulable domain

#### Scenario: Make a join depend on every branch

- **WHEN** two branch domains feed a join task
- **THEN** the join's domain depends on both branches and cannot start until both complete

#### Scenario: Consume while a combined node remains active

- **WHEN** a combined node has graph-local stream consumers
- **THEN** those consumers can progress before the node returns its Task result

### Requirement: Preserve node-kind state ownership

Ordinary tasks SHALL execute serially within their domain. An EventNode instance SHALL receive callbacks serially. Stateful pure producers and combined nodes SHALL serialize their invocations within one streaming instance while independent domains and producers progress. Producer or live-export backpressure MUST NOT occupy all execution capacity required by downstream domains, timers, or export consumers. Separate invocations SHALL retain isolated contexts.

#### Scenario: Serialize one event node's callbacks

- **WHEN** input and timer events become ready for the same event node
- **THEN** its callbacks run in one defined serial order while unrelated domains can progress

#### Scenario: Serialize one producer's invocations

- **WHEN** multiple messages are ready for the same stateful stream producer
- **THEN** one invocation finishes before the next begins on that producer instance

#### Scenario: Keep downstream progress under producer backpressure

- **WHEN** a producer blocks while its output queue is full
- **THEN** the runtime retains capacity for downstream domains that can release the queue

#### Scenario: Fill a combined output queue with one task worker

- **WHEN** a combined node emits more records than capacity with one ordinary Task worker configured
- **THEN** consumers drain records in order and cancellation can interrupt any blocked send

## ADDED Requirements

### Requirement: Publish combined terminal completion independently of stream drain

A combined node's validated return SHALL publish terminal outputs and permit ordinary success dependencies in its originating frame. Terminal publication MUST NOT wait for all admitted stream values to be acknowledged. Whole-workflow success SHALL still wait for connected stream delivery and downstream drain. Publication failure observed before terminal commit MUST suppress Task success.

#### Scenario: Continue after the model returns

- **WHEN** a combined model node returns a valid answer while admitted live records are still draining
- **THEN** its terminal dependent can start and workflow success waits for both paths

### Requirement: Preserve scoped combined invocation identity

Finite Loop and Iteration execution SHALL retain ordinary Task ordering, limits, commits, and cancellation while routing explicitly exported combined emissions. Export routing SHALL retain the complete static node path and dynamic iteration identity through context forks without exposing parent application outputs to child workflows.

#### Scenario: Observe parallel iteration items

- **WHEN** different Iteration items invoke the same combined body node
- **THEN** each live stream has a distinct item/invocation identity and ordinary result ordering is preserved
