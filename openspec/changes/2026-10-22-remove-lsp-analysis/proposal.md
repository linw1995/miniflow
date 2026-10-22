## Why

Test planning carried two exact-narrowing mechanisms: LSP semantic evidence plus test-body isolation, and the coverage graph. The coverage graph subsumes both with measured, conservative coverage attribution. Maintaining the language-server provider, its session machinery, and the isolation classifier costs complexity without changing selections. Removing them leaves one exact mechanism and a Task-only planning path.

## What Changes

- Remove the `mfn-lsp` crate and the `lsp.analyze` node; the planning Flow connects `git.diff` directly to `test_affected.plan`, with the coverage-graph outcome merged into the comparison context beforehand.
- Remove test-body isolation (`classify.rs`, `isolated_tests`); coverage-graph narrowing is the only exact-selector mechanism. The analyzer-failure full-workspace fallback disappears with the analyzer; unknown-input fallback is unchanged.
- `git.diff` drops the analysis request, snapshot documents, and byte ranges; the base-side line intervals used by graph queries remain. Plan output drops the analysis status/timing/relationship counters; saved plans from older planners remain consumable.
- CI/Nightly coverage steps, codecov flags, `run-cov.sh`, and `coverage_selection.py` lose their `mfn-lsp` special cases.

## Capabilities

### Removed Capabilities

- `lsp-analysis`: the language-server Stream node, its protocol handling, and its evidence contract are deleted with the crate.

### Modified Capabilities

- `test-affected`: the planning path is Task-only (Git comparison feeding the planner directly); test-body isolation and analysis-outcome validation are removed; precision gates coverage-graph narrowing only.
- `ci-test-selection`: unknown inputs retain full-workspace fallback; the analyzer-failure fallback no longer exists.
- `development-tool-isolation`: one development tool package remains; language-server-specific validation and release scenarios are removed.
