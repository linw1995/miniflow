## Why

Semantic test refinement narrows a PR selection below package scope only when the diff is isolated to test bodies; any production-code change retains the whole package plus its reverse dependencies, so recent core-crate PRs executed nearly the full suite.
LSP call-chain evidence is collected but never consumed for selection, and extending it into transitive reachability cannot meet the completeness bar (dynamic dispatch, disabled macro expansion, one-hop budget). Measured per-test coverage provides that completeness from real executions, and the existing main coverage runs already produce the raw signal.

## What Changes

- Collect per-test line coverage during successful default-branch coverage runs and index it into an immutable, versioned SQLite graph keyed by commit: file line intervals to distinct tests, bound to the producing run and execution signature.
- Publish the graph through the existing validation/coverage artifact pipeline; only successful default-branch workflows publish. No external database service, release mirror, or new trust boundary is introduced.
- During PR planning, resolve the nearest compatible default-branch ancestor graph for the comparison base, map changed line ranges to exact test selectors, and conservatively widen unmappable, unmapped, or graph-lagging changes to the existing package and reverse-dependency scope. Unknown inputs and analyzer failures retain full-workspace fallback.
- Record graph provenance (commit, run, schema version) and widening reasons in the plan; preserve empty-selection and plan-count semantics. Local planning acquires the same artifact through existing authenticated access.
- Keep `lsp.analyze` unchanged as health gate and fallback trigger; its evidence remains non-normative for selection.

## Capabilities

### New Capabilities

- `coverage-graph`: per-commit test coverage graph collection, storage, publication, and conservative query for selection.

### Modified Capabilities

- `test-affected`: the plan narrows package scopes to exact test selectors through a compatible coverage graph before falling back to package and reverse-dependency scope.
- `ci-test-selection`: default-branch coverage runs publish the graph; PR selection may execute graph-derived exact selectors; record compatibility covers the graph schema.

## Non-goals

No transitive LSP call-chain selection, external or hosted database, unauthenticated mirror, per-PR graph publication, graph-driven coverage-gate changes, or scheduler changes. The graph narrows which tests run; it never weakens the incremental coverage gate, baseline advancement rules, or shared-input policy. Exact-selector execution support in the coverage consumer is a prerequisite, not a deliverable.
