# Verification

## Focused checks

- OpenSpec strict validation passed for `add-stream-node-execution`.
- The reviewed producer, stream planning, execution, capacity, observation, and context suites passed: 45 tests across six binaries.
- The final producer, context, and external runner checks passed: 18 tests across three binaries.
- The external runner matches in-memory execution for a 205-line file with CRLF, an empty line, retained whitespace, Unicode, and no final newline. Empty files, empty stdin, missing files, and invalid UTF-8 are covered.
- Producer observation verifies worker span propagation, successful emission counts, publication failures, and execution failures.

## Repository checks

- `nix develop --command prek -a` passed every hook, including Clippy, Rust diagnostics, formatting, Markdown, workflow checks, and the license audit.
- `nix flake check -L` passed release packaging, formatting, Clippy, workflow validation, and all 356 tests across 62 binaries, with zero skipped tests.
- The final suite includes eleven producer tests and one external runner test. Shared context, port, synchronous-scope, and worker-pool tests cover their common contracts.
- The full Nix log is local at `target/validation/flake-check-review.log`.
- Post-archive OpenSpec validation passed for all 16 main specifications and all 19 archived changes.

## Review

- A cleanup deadlock was reproduced: joining a producer under the scheduling mutex blocked its destructor from checking the closed input handle. Worker joining and plugin destruction now happen outside that mutex; the regression passes.
- Task and producer pools remain independently owned. Shutdown joins those pools before terminal state cleanup and observation, including coordinator unwinding.
- Completion records preserve dispatch identity because a task paused before a producer and a completed producer can have the same frame cursor.
- Each emitter borrows the instance and plan, publishes one validated result through the common queue path, and preserves instance failure when a plugin ignores a send error.
- Skipped inputs are covered alongside FIFO and chained production using the real conditional node. Empty production is covered by the external empty-file case. Common port and synchronous-scope contracts retain their existing tests.
- Preparation remains free of producer operations; synchronous placement and cross-domain dependencies are rejected before execution.

## Limits

- Queue bounds count messages. Individual payloads and plugin-owned buffers have no byte quota.
- Producer threads are additional to `execution.limits.workers` and are bounded by the graph's stream-node count.
- Cancellation interrupts runtime send waits. Arbitrary synchronous plugin I/O must return before cleanup can finish.
- Consumers with exhaustive matches on `NodeExecution` or `StreamError` must handle the added variants and rebuild against the matching runtime identity.
