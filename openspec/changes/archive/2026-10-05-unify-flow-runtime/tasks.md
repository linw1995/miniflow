# Tasks

## 1. Unified Flow and domain planning

- [ ] 1.1 Add execution mode, message-domain metadata, and execution-domain DAGs to the common prepared Flow; verify oneshot and stream produce the same plan type and incompatible node kinds fail before invocation.
- [ ] 1.2 Partition validated data/control graphs into maximal serial domains, splitting at forks, joins, and event/stream boundaries; verify every node is assigned once for chains, fan-outs, fan-ins, independent roots, and control edges.
- [ ] 1.3 Introduce `FlowRuntime` over the common plan and route in-memory oneshot and stream APIs through it; verify both modes use the same domain scheduler and preserve lifecycle results.
- [ ] 1.4 Route generated oneshot and stream runners through `FlowRuntime`, retaining compatibility facades where practical; verify generated and in-memory execution agree for both modes.

## 2. Synchronous domain execution and bounded concurrency

- [ ] 2.1 Implement ordered synchronous execution within one domain and bounded concurrent dispatch across independent ready domains; verify chain nodes never overlap, fork domains overlap, join domains wait, and a worker limit of one preserves topological order.
- [ ] 2.2 Add isolated domain contexts and completion records for node outputs, skips, Loop writes/exits, snapshots, and observations, with coordinator-owned step accounting; verify sibling domains cannot observe uncommitted state and each effect commits once.
- [ ] 2.3 Apply Loop scope effects in stable domain order and stop dispatch beyond a committed exit while draining started domains; verify Loop pass, early-exit, and scope-write behavior.
- [ ] 2.4 Integrate domain execution into nested Loop and Iteration bodies using the shared worker budget; verify nested fan-out progresses without exhausting workers or changing Iteration result order.
- [ ] 2.5 Preserve EventNode callback serialization and per-instance StreamNode serialization while allowing independent domains and producers to progress; verify Send-only event/producer states remain supported.
- [ ] 2.6 Add `max_parallel_domains` defaults and overrides, composing them with existing stream worker limits; verify active domains never exceed the effective limit and ready queues remain bounded.

## 3. Stream ordering, progress, and failure behavior

- [ ] 3.1 Schedule execution domains within each stream frame while preserving message-domain identity and per-message-domain FIFO; verify forked domains share one message identity, joins wait for all branches, and slow earlier frames are not overtaken.
- [ ] 3.2 Preserve timer, producer, and downstream progress under full queues; verify a producer blocked on emission capacity cannot occupy the worker needed by its downstream domain.
- [ ] 3.3 Stop domain dispatch on failure, drain started calls, attribute failures, and suppress late publications; verify cancellation wakes blocked senders and completed stream output prefixes remain intact.
- [ ] 3.4 Verify oneshot completion and stream close/drain with concurrent domains, including selected outputs completing out of order and empty or skipped branches.

## 4. Compatibility, documentation, and integration

- [ ] 4.1 Expose `max_parallel_domains` in runtime APIs and generated runner entry points; document the default, stream cap, domain partitioning, and independent-domain side-effect ordering.
- [ ] 4.2 Migrate compiler and runtime examples, tests, and node-development docs to the common Flow/domain contract; verify existing workflow definitions require no schema migration.
- [ ] 4.3 Run OpenSpec validation, `nix develop --command prek -a`, and `nix flake check -L`; review oneshot/stream parity, nested-flow behavior, bounded progress, and compatibility facades before archiving.
