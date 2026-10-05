# Tasks

## 1. Bounded Worker Pool

- [x] 1.1 Add a non-owning worker handle and same-thread help operation; verify pool workers can drain nested jobs without spawning threads.
- [x] 1.2 Add bounded `run_parallel` submission and panic draining; verify nested jobs complete when all pool workers submit children and observed worker thread count never exceeds the configured limit.
- [x] 1.3 Preserve zero-worker, full-queue ownership, startup cleanup, and drop-and-join behavior; run all `mf-runtime::worker` unit tests.

## 2. Oneshot Domain Scheduling

- [x] 2.1 Share immutable prepared task Flow data through `Arc`; verify worker jobs hold cheap plan handles without cloning node or dependency vectors.
- [x] 2.2 Dispatch ready oneshot domains through the bounded runtime pool; verify fork/join, one-worker ordering, failure draining, panic propagation, and rendezvous concurrency tests.
- [x] 2.3 Keep the execution coordinator responsible for readiness and commits; run `nix develop --command cargo test -p mf-runtime -p mf-compiler -p mfn-core -j 2`.

## 3. Stream Domain Scheduling

- [x] 3.1 Route stream domain jobs through the same runtime pool and attach its non-owning handle to frame contexts; verify stream worker and runtime limits combine by minimum.
- [x] 3.2 Preserve dedicated producer lanes, message-domain FIFO, backpressure, cancellation, and completion delivery; run stream capacity, execution, producer, and parity integration tests.

## 4. Parallel Iteration

- [x] 4.1 Replace Iteration's scoped threads with bounded work submitted through `ExecutionContext::run_parallel`; verify nested items make progress with one worker and the pool creates no extra threads.
- [x] 4.2 Preserve the ten-item cap, item isolation, result order, panic draining, and all item error policies; run `nix develop --command cargo test -p mf-compiler --test iteration -j 1`.
- [x] 4.3 Remove `DomainBudget`, permit handoff methods, and their redundant tests; verify no domain execution path uses a separate concurrency counter.

## 5. Integration

- [x] 5.1 Update runtime and workflow documentation to describe the shared worker bound; verify examples and generated runners use the same setting in both modes.
- [x] 5.2 Run `nix develop --command prek -a`, `openspec validate --all`, and `nix flake check -L`; resolve any failures and re-run affected checks.

## Workflow follow-up

- Archive the change after the implementation and specification review are complete.
- Verify the archived change and monitor the updated PR checks and review comments.
