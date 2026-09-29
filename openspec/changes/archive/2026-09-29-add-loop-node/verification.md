# Verification

The scenarios below are exercised by the named tests. The complete workspace suite also runs through `nix flake check -L`. Coverage reports remain local under `target/coverage/result/`.

## Workflow Loop execution

| Scenario | Check |
| --- | --- |
| Initialize and expose loop variables | `loop_planning::executes_loop_with_persistent_state_in_memory`; `loop_planning::generated_loop_matches_in_memory_without_build_inputs` |
| Reject an invalid definition version | `loop_planning::rejects_old_schema_and_invalid_loop_structure` |
| Reject a body cycle or cross-scope reference | `loop_planning::rejects_old_schema_and_invalid_loop_structure` |
| Validate an inactive body branch | `loop_planning::validates_body_plugins_and_loop_port_types` |
| Reject assignment outside a Loop | `loop_planning::rejects_old_schema_and_invalid_loop_structure` |
| Refine a value across passes | `loop_planning::executes_loop_with_persistent_state_in_memory` |
| Do not reuse a stale body output | `loop_planning::later_pass_cannot_read_an_omitted_prior_output` |
| Preserve state through a skipped assignment | `loop_planning::skipped_assignment_preserves_previous_value` |
| Stop after a condition becomes true | `loop_planning::loop_observation_identifies_each_pass_and_body_invocation` |
| Stop at the maximum | `loop_planning::stops_at_maximum_and_immediate_exit` |
| Prefer the condition at the maximum boundary | `loop_planning::loop_observation_identifies_each_pass_and_body_invocation` |
| Exit from a branch | `loop_planning::stops_at_maximum_and_immediate_exit` |
| Skip the entire Loop | `loop_planning::skips_the_whole_loop_when_a_control_dependency_is_inactive` |
| Exhaust a nested execution budget | `loop_planning::nested_loops_restore_parent_state_and_share_step_budget`; `loop_planning::generated_loop_matches_in_memory_without_build_inputs` |
| Fail on an invalid assignment value | `loop_planning::invalid_assignment_does_not_publish_partial_loop_output` |

## Workflow binary compilation

| Scenario | Check |
| --- | --- |
| Compile a valid workflow | `loop_planning::generated_loop_matches_in_memory_without_build_inputs`; `packaged_cli::packaged_cli_acceptance` |
| Run a compiled Loop workflow | `loop_planning::generated_loop_matches_in_memory_without_build_inputs` |
| Run without build inputs | `packaged_cli::packaged_cli_acceptance` |
| Reject a body cycle before code generation | `loop_planning::rejects_old_schema_and_invalid_loop_structure` |
| Reject an invalid body plugin before installation | `loop_planning::validates_body_plugins_and_loop_port_types` |
| Preserve an existing binary after invalid Loop edit | `loop_planning::invalid_loop_edit_preserves_installed_binary_and_lock` |

## Workflow observability

| Scenario | Check |
| --- | --- |
| Observe repeated invocations | `observation::execution_matches_unobserved_results_with_skips_and_native_span_parentage` |
| Correlate a node event | `loop_planning::loop_observation_identifies_each_pass_and_body_invocation` |
| Preserve a single execution lifecycle | `observation::failures_report_real_phases_and_never_publish_a_success_first` |
| Correlate repeated body invocations | `loop_planning::loop_observation_identifies_each_pass_and_body_invocation` |
| Keep transport duplicates distinct from repeated work | `state::duplicate_delivery_and_a_new_pass_have_distinct_identities` |
| Detect an interior loss | `state::interior_gap_and_trace_only_loss_have_distinct_integrity` |
| Detect missing tail evidence | `state::final_prefix_and_missing_tail_keep_unknowns_explicit` |
| Receive no telemetry | `tui_run::tui_preserves_stdout_and_restores_terminal_after_missing_telemetry` |
| Preserve uncertainty for missing node outcomes | `state::a_late_body_outcome_closes_the_gap_without_inventing_success` |
| Identify unreached nodes with bounded terminal metadata | `state::visited_prefix_uses_execution_order_and_final_failure_identity`; `state::early_exit_marks_only_the_unvisited_body_suffix_not_run` |
| Detect a dropped body event | `state::a_late_body_outcome_closes_the_gap_without_inventing_success` |
| Stop before a future pass | `loop_planning::loop_observation_identifies_each_pass_and_body_invocation` |
| Keep missing tail evidence unknown | `state::final_prefix_and_missing_tail_keep_unknowns_explicit` |
| Describe a nested Loop | `loop_planning::nested_loops_restore_parent_state_and_share_step_budget`; `contracts::nested_descriptions_validate_scope_ownership_and_version` |
| Observe Loop termination | `loop_planning::loop_observation_distinguishes_exit_skip_and_nested_paths` |

## Workflow terminal UI

| Scenario | Check |
| --- | --- |
| Inspect live refinement | `state::live_body_start_shows_running_in_the_active_pass`; `graph::loop_body_layout_renders_the_selected_pass_independently` |
| Retain bounded history | `state::loop_passes_keep_independent_node_outcomes_and_bounded_history`; `run::tests::loop_view_enters_body_and_returns_to_root` |
| Keep a missing earlier outcome unknown | `state::a_late_body_outcome_closes_the_gap_without_inventing_success` |

## Gates

- `nix develop --command prek install`
- `nix develop --command prek -a`
- `nix flake check -L`: 236 tests passed on `aarch64-darwin`; the flake reported other systems as incompatible with this host.
- `nix develop --command bash scripts/run-cov.sh -E 'test(loop) | test(state) | test(observation)'`: 23 selected tests passed; local reports were written to `target/coverage/result/`.
- `openspec validate add-loop-node --strict`
