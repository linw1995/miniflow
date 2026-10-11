## Acceptance Evidence

| Delta scenario | Evidence |
| --- | --- |
| Separate parallel data and control edges | `parallel_data_and_control_edges_use_separate_block_rows`; `benchmark_routes parallel 64` completes all 128 connections. |
| Route around an intervening node | `long_edges_use_passages_and_keep_isolated_components_disjoint`; `directional_search_finds_an_obstacle_detour_with_few_bends`. |
| Preserve branch labels and arrows | `control_labels_are_sanitized_truncated_and_reserved_in_cell_units`; `crossing_validation_rejects_corners_labels_and_third_participants`; `completed_routes_leave_and_enter_ports_horizontally`. |
| Exceed four incident edges | `crowded_attachments_expand_nodes_and_channels_without_reusing_rows`; `crowded_inputs_have_distinct_rows_and_disjoint_boxes`. |
| Exceed the initial channel capacity | The crowded attachment tests assert expanded column gaps and validate complete route geometry; ordered tracks are reused only between disjoint interval regions. |
| Display both crossing orientations | `crossings_preserve_each_strokes_kind_color_and_modifier`; the inspected `graph_preview -- --crossings` matrix and mixed crowded previews. |
| Move a crowded crossing | `crossing_validation_rejects_corners_labels_and_third_participants`; `local_rerouting_never_replaces_a_better_valid_arrangement`. |
| Change execution state at a crossing | `crossings_preserve_each_strokes_kind_color_and_modifier`; `input_permutations_keep_geometry_crossing_layers_and_rendered_styles`; the live PTY preview applies produced/skipped branch outcomes. |
| Pan across a partially visible marker | `partially_visible_crossings_match_full_canvas_symbols_and_styles`; `clipped_tall_nodes_and_wide_labels_match_the_full_canvas`; `a_wide_label_fragment_cannot_overwrite_the_next_pane` blanks a partial wide glyph at the right boundary. |
| Reorder connection descriptions | Eight data/control permutations in `input_permutations_keep_geometry_crossing_layers_and_rendered_styles` compare canonical geometry and complete rendered buffers. |
| Exhaust a routing budget | `routing_budgets_retain_all_connections_without_unsafe_paths`; `search_and_conflict_limits_stop_before_exceeding_counters`; the dense benchmark retains explicitly unresolved connections. |
| Inspect an unresolved connection | `unresolved_connections_keep_inspectable_kind_ports_and_reason`; `routing_warnings_are_independent_of_observation_integrity`; `details_window_wraps_cell_width_and_reaches_rows_beyond_u16`. |
| Inspect multiple passes of a body | `root_and_loop_body_share_routing_geometry`; `loop_views_preserve_navigation_and_compacted_stream_totals` switches passes and stream owners while comparing cached routes. |

## Local Artifacts

`target/edge-routing/` retains crossing prototypes, sparse and crowded root/body text exports, inspected Menlo PNGs, raw timing and resident-memory reports, and a live PTY capture. The interactive preview completed its simulated workflow, accepted panning/origin keys, and exited normally with cursor and alternate-screen restoration. These artifacts remain Git-ignored.

## Validation Commands

- `nix develop --command cargo test -p mf-tui --lib`
- `nix develop --command cargo test -p mf-tui --test graph`
- `nix develop --command cargo test -p mf-cli --test tui_run --test stream_tui`
- `nix develop --command bash scripts/run-cov.sh -E 'package(mf-tui)'`
- `nix develop --command prek install`
- `nix develop --command prek -a`
- `nix flake check -L`
- `openspec validate route-tui-edges-without-overlap --strict`

The implementation passes 41 library tests, 20 graph integration tests, nine existing CLI TUI tests, all repository hooks, and strict OpenSpec validation. The scoped coverage workflow passes 95 TUI tests and writes `lcov.info` under `target/coverage/result/`. The complete Nix workspace test suite passes 560 tests with two existing skipped tests. The complete `nix flake check -L` succeeds on aarch64-darwin, including package build, formatting, Clippy, workspace tests, and workflow validation.

## Ablation and Final Review

The selected crossing renderer now uses a five-cell array and skips offscreen marker footprints.
Removed the unused crossing-style enum, bridge prototype, duplicate point-count validation, primitive
footprint test, and immutable-render assertions. All 90 structured and seeded fixture variants preserve
routing results and work counters. Search, local retries, interval pruning, and validated outer detours
remain supported by quality or performance evidence. Local reports and temporary sources remain under
ignored `target/edge-routing/ablation/`.

The repository-instruction audit and final review confirm existing module boundaries, typed Snafu sources, deterministic route ownership, safe completed geometry, and separate unresolved-routing diagnostics. The PR uses the repository template with AI Disclosure.

The delta passed strict validation before archive. Post-archive canonical validation passes without strict mode. Full strict canonical validation retains the same pre-existing long-requirement warnings as `origin/main` across all 21 specs; the six added routing requirements introduce no warnings.
