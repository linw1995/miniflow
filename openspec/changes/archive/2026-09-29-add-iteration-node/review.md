# Completion review

## Decision

The Iteration change is ready to archive. The implementation, generated runner, observation behavior, and documented scope match the three spec deltas. No blocking gap remains after the commit-by-commit ablation review.

## Requirement traceability

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Scoped item and index binding | `mf-compiler/src/iteration.rs`; `mf-runtime/src/context.rs` and `iteration.rs` | `iteration_collects_values_in_input_order_and_isolates_item_contexts`; `iteration_rejects_invalid_body_structure_and_result_ports` |
| Bounded scheduling and ordered results | `mf-runtime/src/iteration.rs` | `parallel_mode_is_bounded_and_returns_input_order`; sequential and parallel cases in `iteration_collects_values_in_input_order_and_isolates_item_contexts` |
| Item failure policies | `mf-runtime/src/iteration.rs` | `iteration_error_policies_preserve_positions_or_remove_failures`; skipped-result and observed-terminate cases |
| Validated standalone body generation | `mf-compiler/src/compiler.rs` and `plan.rs` | `generated_parallel_runner_matches_in_memory_and_describes_one_iteration_node`; existing validated-install regression tests |
| Correlated item and body-node observation | `mf-telemetry/src/observation.rs`; shared execution in `mf-runtime/src/context.rs` | `parallel_iteration_reports_correlated_item_and_body_node_activity`; `body_plugins_inherit_the_correlated_node_span`; generated-runner observation assertions |
| Sampling, skip, and outer lifecycle isolation | `mf-telemetry/src/observation.rs`; `mf-tui/src/receiver.rs` | Unsampled detail-log, skipped-body-node, and `iteration_detail_logs_leave_outer_lifecycle_complete` tests |

## Scope and boundaries

One iteration level is supported. Body nodes share prepared `Send + Sync` instances, while each item has a fresh execution context. Outer workflow values require explicit representation in the input array. The terminal UI describes the outer Iteration node; the separate `mf.iteration` OTel scope carries item and body-node detail. Export remains bounded and best effort. These boundaries are documented in `docs/workflows.md` and `docs/observability.md`.

## Validation

- `openspec validate add-iteration-node --strict`: passed.
- `nix develop --command prek -a`: passed.
- `nix develop --command bash scripts/run-cov.sh`: 217/217 tests passed; reports remain under ignored `target/coverage/`.
- `nix flake check -L`: 217/217 tests passed on aarch64-darwin.

The ablation report and experimental backups remain under ignored `target/ablation/` and are excluded from commits.
