# Design

## Context

See [proposal.md](proposal.md) for motivation. The coordinator invokes event callbacks while holding shared scheduling state. Ordinary tasks run on a bounded worker pool. Event outputs establish separate FIFO message domains and enter per-operator pending queues.

## Goals / Non-Goals

**Goals:** Preserve these scheduling boundaries while allowing a producer to yield any finite number of outputs with bounded queue occupancy. Keep one mutable executor per workflow instance and preserve existing task and event behavior.

**Non-Goals:** Automatic source startup without `%input`, file parsing, interrupting arbitrary synchronous plugin I/O, payload byte quotas, and changing the event callback contract.

## Decisions

### Add a separate stream executor

Add `StreamNode: Send` with `execute(&mut self, Inputs, &mut ExecutionContext, &mut Emitter<'_>) -> Result<(), NodeExecutionError>`, `NodeExecution::Stream`, and `PreparedNode::stream`. Returning successfully ends the current input invocation. Metadata remains available without executing the node. Synchronous scopes accept only tasks.

This keeps stateful timers and bounded batch emissions on the existing event interface. Returning a lazy iterator was considered, but explicit sends let plugins retain ordinary control flow and use blocking I/O naturally.

### Own one producer worker per used stream node

Lazily initialize a one-thread `WorkerPool` for each stream node. Retain it across inputs and serialize access to its Send-only executor. Producer jobs hold the current input frame until execution returns. Completions identify their dispatch kind: a task can stop immediately before a producer, while a producer returns at that same cursor after executing it. The number of producer threads is bounded by the prepared graph's stream-node count.

Using the shared task pool could deadlock when every worker is a blocked producer. Blocking on the coordinator would prevent output consumption and timer progress. Dedicated producer workers avoid both dependencies while reusing the pool's thread ownership and shutdown behavior.

### Send through the existing pending queue

The emitter borrows the instance state and plan for one invocation. Each send waits on the runtime condition variable until the node's pending queue has capacity, then validates and publishes one `NodeResult` with a fresh output-domain identity. Waiting releases the scheduling mutex. Each dequeue and terminal failure wakes senders. A successful send transfers ownership to the runtime; it does not wait for downstream processing.

Event callbacks and stream producers share single-emission validation and queue insertion. Event callbacks separately apply their vector capacity check and timer updates.

The emitter holds no full-input output buffer. Invalid output fails the instance even if a plugin ignores the returned send error. Existing event-vector limits remain unchanged.

### Retain input frames through producer completion

The producer's upstream domain stays occupied until the invocation completes, while its separate output domain consumes emitted messages independently. Later inputs cannot overtake an earlier invocation. Upstream closure waits for admitted producer jobs; an output domain closes after production has ended and queued and in-flight emissions have drained. Empty and skipped invocations do not invent output messages.

### Integrate cancellation and observation

Failure wakes blocked sends and prevents further publication. The coordinator stops dispatching, then joins the owned task and producer pools. These joins settle in-flight calls before final state cleanup and terminal observation. Dropping an unfinished instance uses the same path. Arbitrary plugin I/O remains cooperative, just like existing synchronous tasks.

Workers and plugin state are dropped outside the scheduling mutex, so cleanup can use closed or failed runtime handles. Coordinator panics record and broadcast failure before joining workers. Ordinary task concurrency retains its dispatch counter; producer shutdown relies on worker ownership and joining.

Use the existing input-triggered observation callback for each producer invocation, with its input message identity, emitted count, and produced ports. Propagate observation context onto producer threads. Distinguish output validation failures from plugin execution failures. Generated streaming runners use the same runtime implementation.

## Risks / Trade-offs

- More stream nodes require more producer threads. Lazy startup bounds overhead to nodes that execute; a separately bounded producer scheduler can be considered if large graphs need it.
- Message limits do not bound individual payload sizes or plugin buffers. Document this alongside the existing capacity contract.
- A plugin can block in external I/O or ignore send errors. Cancellation interrupts runtime capacity waits, and cleanup waits for plugin calls to return.
- Extending the public execution enum requires consumers with exhaustive matches to rebuild and update those matches.

## Migration Plan

Rebuild runtime consumers and plugins against the same runtime identity. Existing task and event factories keep their execution contracts. New producers opt in through `PreparedNode::stream`; synchronous placement fails during preparation. No workflow JSON migration is needed.
