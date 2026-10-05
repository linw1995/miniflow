# Proposal

## Why

The compiler already validates one workflow graph, but then converts it into separate task-only and streaming execution plans with different schedulers. One-shot execution also runs independent ready nodes serially, leaving ordinary DAG fan-out without a way to use bounded parallelism.

## What Changes

- Build one executable `Flow` representation for both oneshot and stream modes and execute it through a shared runtime and dependency scheduler.
- Partition each Flow into synchronous execution domains: nodes run serially within a domain, while independent ready domains run concurrently up to a bounded runtime limit.
- Keep execution domains distinct from stream message domains so oneshot and stream share scheduling without changing message identity or lifecycle rules.
- Give concurrent domains isolated execution contexts and commit validated results and scoped effects before scheduling dependent domains.
- Preserve mode-specific lifecycles: oneshot completes one graph invocation; stream retains event handling, sources, message domains, backpressure, and drain behavior.
- **BREAKING**: Independent domains may complete and perform external side effects in a different order. Dependency order, deterministic dispatch priority, and selected output ordering remain defined.

## Capabilities

### New Capabilities

- `workflow-runtime-execution`: Prepare one executable Flow and run oneshot and stream workflows through a shared, bounded scheduler.

### Modified Capabilities

- `node-preparation`: Define domain-local execution contexts and staged effects so concurrent domains do not share mutable run state.
- `workflow-stream-execution`: Allow independent execution domains in one message frame to run concurrently while preserving message-domain FIFO, event serialization, and bounded progress.

## Impact

This affects `mf-runtime` Flow preparation, execution contexts, worker scheduling, stream coordination, `mf-compiler` mode selection and generated runners, the public runtime API, runtime observations, and workflow/runtime documentation. Stream definitions keep their explicit mode and existing lifecycle limits; no new dependency is planned.
