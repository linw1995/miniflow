## 1. Feasibility spike

- [x] 1.1 Emit per-test LLVM profiles on a default-branch coverage run; measure added wall time and export cost against the existing coverage job budget.
- [x] 1.2 Build a prototype interval-compressed SQLite graph from the export; measure file size and changed-range query latency; record results under target/ and decide go/no-go.

## 2. Graph store

- [x] 2.1 Add the versioned SQLite schema (records, tests, files, interval coverage) in the existing unpublished tooling package with the workspace Diesel dependency, typed errors, and query-only readers.
- [x] 2.2 Implement graph indexing from per-test coverage exports with commit, run, and execution-signature binding; extend existing verification and commit this group.

## 3. Default-branch generation and publication

- [x] 3.1 Extend the main coverage job to collect per-test profiles, build the graph, and publish it through the existing artifact pipeline only after successful full coverage.
- [x] 3.2 Bind graph compatibility (schema version, execution signature) into record checks; verify PR and local runs cannot publish; commit this group.

## 4. Planning consumption

- [ ] 4.1 Resolve the nearest compatible default-branch ancestor graph through existing lazy bounded acquisition, shared by CI and local planning.
- [ ] 4.2 Map changed ranges to exact selectors with the three amplification rules (graph-lagging files, uncovered ranges, absent files) and record provenance and reasons in the plan.
- [ ] 4.3 Preserve existing tiers and fallbacks; verify graph-derived selectors through both exact-filter consumers, including widen-on-mismatch; commit this group.

## 5. Review and archive

- [ ] 5.1 Ablate amplification rules and provenance fields; remove unnecessary abstractions; run focused tests and required hooks.
- [ ] 5.2 Compare selection size and duration against package-level selection on representative historical PRs; synchronize docs/ci.md and AGENTS.md; archive in a separate commit.
