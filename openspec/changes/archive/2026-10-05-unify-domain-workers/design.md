# Design

## Context

The runtime currently has three parallel-work paths: `Flow::execute_domains` creates scoped threads, stream domains use `WorkerPool<DomainJob>`, and `mfn-core` Iteration creates another set of scoped threads. `DomainBudget` shares a permit count across forked contexts, but it does not own or bound those OS threads. Stream producers use separate per-producer workers to keep blocked sends away from downstream domain capacity.

## Goals / Non-Goals

**Goals:**

- Use one bounded worker-pool implementation for oneshot domains, stream domains, and parallel Iteration items.
- Make the configured worker count bound runtime worker threads, including nested Iteration work.
- Allow synchronous nested work to make progress when every worker is already inside a parent task.
- Keep domain coordinators responsible for readiness, context commits, stream frame ordering, and failure policy.

**Non-Goals:**

- Merge producer-specific lanes into the domain queue; a blocked producer must not consume all downstream capacity.
- Change graph partitioning, stream message identity, Loop scope ordering, Iteration's ten-item cap, or public workflow schemas.
- Add an async runtime or allow concurrent callbacks on one stateful node instance.

## Decisions

### One runtime pool for domain and Iteration jobs

Oneshot owns a pool for the duration of a top-level run. A stream instance owns its domain pool for the instance lifetime. Both use the same `WorkerPool<WorkerJob>` implementation and configured worker limit; stream uses the minimum of `RuntimeOptions.max_parallel_domains` and `execution.limits.workers`. Iteration submits bounded item-worker jobs through the same pool instead of creating threads. Its existing ten-item limit remains an upper bound.

Producer execution remains on its existing per-producer `WorkerPool`. Those lanes use the same worker-pool implementation but remain separate queues so a producer blocked by backpressure cannot starve downstream domain work.

### Cooperatively help nested work

Each pool worker can submit child jobs and wait while helping execute queued jobs from the same pool. Helping uses the current OS worker thread; it does not spawn a compensation thread. A non-worker coordinator waits for completion without helping, so it cannot exceed the pool's thread count. The queue remains bounded; a worker that encounters a full queue helps drain ready work before retrying submission.

This resolves pool exhaustion for nested Iteration: workers blocked in parent Iteration calls run available item jobs, and each item can recursively help with its own nested work. A pool without helping was rejected because all workers could block waiting for child jobs queued to that same pool. Keeping `DomainBudget` alongside separate thread creation was rejected because it limits admitted work but not the number of worker threads.

### Share immutable Flow plans with worker jobs

Pool jobs need owned, `'static` captures, while the public execute APIs borrow `Flow`. Store the prepared immutable task plan behind `Arc` and clone that handle into each domain job. This avoids cloning the full node and dependency vectors for every dispatch and keeps task executors shared under their existing `Send + Sync` contract.

### Keep worker handles non-owning in contexts

`ExecutionContext` carries a weak worker handle through domain and body forks. The top-level oneshot call or stream instance owns the pool and controls shutdown. A context cannot keep the pool alive after its owner finishes, and queued jobs cannot form an ownership cycle through their captured contexts.

### Preserve coordinator completion behavior

One-shot and stream coordinators still decide which domains are ready and commit completions. Domain jobs catch panics, return isolated contexts, and leave failure selection and output publication with the coordinator. Iteration workers collect indexed results, drain already-started jobs after failure or panic, and preserve input order before applying the node's failure policy.

## Risks / Trade-offs

- A helper can execute a different ready domain while waiting for nested work → keep dependency admission in the coordinator; only already-admitted jobs enter the pool queue.
- Worker-job panics could strand completion waiters → wrap every submitted domain and item job, report completion exactly once, then resume the selected panic after started work drains.
- Contexts can outlive their pool owner → use weak handles and fall back to a local bounded pool for direct node invocation without a runtime handle.
- Pool helping changes thread interleaving → keep per-domain node order, per-node state serialization, Loop scope serialization, and Iteration result ordering as explicit tests.

## Migration Plan

1. Add worker helping and bounded `run_parallel` support to `WorkerPool`, with tests for nested progress and worker-count bounds.
2. Move prepared task Flow data behind `Arc`; route oneshot and stream domain dispatch through owned pool jobs.
3. Move parallel Iteration item-worker loops to `ExecutionContext::run_parallel` and remove `DomainBudget` and permit handoff.
4. Run the full runtime, stream, Iteration, generated-runner, and repository checks before archiving this change.

Rollback restores the current scoped-thread oneshot scheduler and `DomainBudget`; serialized workflow definitions and stream APIs do not change.
