# Tasks

## 1. Contracts and preparation

- [x] 1.1 Add the stream executor and emitter API, document their ownership and admission semantics, and verify compiler preparation rejects synchronous placement and cross-domain dependencies.
- [x] 1.2 Extend stream preparation and message-domain planning to retain Send-only producer state, verified by metadata and instance-isolation tests.

## 2. Runtime execution

- [x] 2.1 Implement reusable dedicated producer workers and blocking bounded sends; verify output beyond queue capacity, FIFO order, state reuse, and progress with one task worker.
- [x] 2.2 Integrate producer completion, upstream closure, empty/skipped inputs, and chained operators; verify complete draining without extra source input.
- [x] 2.3 Integrate cancellation, output validation, errors, and panics; verify blocked send wakeups, ignored-error failure, delivered-prefix behavior, and resource release.
- [x] 2.4 Preserve observation context and emission accounting; verify node identity and publication versus execution failure records, and document producer thread and cancellation limits.

## 3. Runner integration and validation

- [x] 3.1 Add an external line-producer fixture and a compiled runner test covering output above capacity, validation without file I/O, and in-memory parity.
- [x] 3.2 Run focused stream tests and repository checks (`prek -a`, `nix flake check -L`), validate OpenSpec, and record results and any environment limitations.

## 4. Review and archive

- [x] 4.1 Review the implementation and tests with isolated ablations and negative controls; retain experiment artifacts only under `target/`.
- [x] 4.2 Fix the reproduced cleanup reentry deadlock and verify shutdown releases the scheduling mutex before joining producer workers.
- [x] 4.3 Re-run repository checks, complete the review, and archive the change with its specification updates.
