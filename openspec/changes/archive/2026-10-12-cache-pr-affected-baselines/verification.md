## Validation

- All 31 script tests passed using the final pinned collector patch.
- The real Rust fixture verifies single execution of selected tests, preserved untouched rows, cumulative coverage against rebuilt binaries, removed-test profile pruning, and recovery of an unadvertised merge commit in a fresh clone.
- Actual Actions glob resolution confirms baseline archives exclude compilation artifacts and PR/main namespaces remain separate.
- Full coverage initially passed all 596 tests with two skipped. After synchronizing main, native coverage and PR collection passed all 601 tests with two skipped and produced the standard reports.
- The first PR snapshot compressed to 15.5 MiB. Initial full PR collection uploads only the patch flag; full project comparisons remain outside PR CI.
- Codecov uploads only the aggregate LCOV file. Automatic discovery previously included internal per-test profiles among 605 uploaded files.
- Repository hooks and aarch64-darwin flake checks passed.

## Ablation and simplification

Four isolated mutations fail: removing commit-bundle import, clearing profiles on every incremental collect, disabling profile pruning, and running selected tests twice. The initial unmodified diff collector selected a fixture consumer in status but recollected no tests. The patch includes explicit rule hits with matching instrumented build parameters.

Use the existing scripts and real fixture. Avoid a second test pass, custom cleanup workflows, baseline archives containing build directories, duplicate input lists, or changing a commit anchor without refreshing mappings. Sparse per-test counters replace rerun tests rather than accumulating obsolete aggregate LCOV hits. Detailed experiment artifacts remain under ignored target directories.

## Operating limits

GitHub handles inactivity and least-recently-accessed eviction. Cache namespaces share the repository storage limit. Missing or evicted PR snapshots fall back to compatible main data or full collection. Only successful runs publish PR snapshots; main never restores them. The pinned tool patch is required for rule-aware collection and optional sparse profile export.
