# Tasks

## 1. Define and validate the iteration graph

- [x] 1.1 Add structural Iteration configuration and the reserved item/index body source.
- [x] 1.2 Validate body graph structure, nested-kind exclusion, ports, result selection, and types before runner installation.
- [x] 1.3 Normalize body node and edge order in the compiled plan.

## 2. Execute each item

- [x] 2.1 Add fresh per-item contexts and shared body execution for in-memory workflows.
- [x] 2.2 Add sequential and at-most-ten-worker parallel scheduling with input-order results.
- [x] 2.3 Add terminate, continue-on-error, and remove-failed policies with contextual item errors.

## 3. Generate and package

- [x] 3.1 Generate static body calls and prepared-node capture in standalone runners.
- [x] 3.2 Add a runnable example, workflow documentation, and in-memory/generated-runner tests.

## 4. Observe repeated execution

- [x] 4.1 Emit item and body-node spans and detail logs with outer node ID, item index, node identity, outcome, and failure context.
- [x] 4.2 Propagate the item context into parallel workers and parent body-node spans to item spans.
- [x] 4.3 Verify in-memory and generated runners, including continuation failures, sampling, skipped body nodes, and terminal UI protocol isolation.
- [x] 4.4 Run `nix develop --command prek install`, `nix develop --command prek -a`, `nix flake check -L`, and `openspec validate add-iteration-node --strict`.
