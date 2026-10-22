## Collection

Extend the default-branch coverage run to emit one LLVM profile per test (nextest
per-test profile files), then export each profile to line coverage and index the
result. Collection cost is the primary feasibility risk: profile-per-test adds
instrumentation I/O and export work proportional to the suite size. Gate the change
on a measured spike on main: added wall time for the full suite and the resulting
graph size must stay within the existing coverage job budget before any consumer
ships. If the spike exceeds budget, stop; the package-level selection remains the
supported behavior.

## Graph store

One immutable SQLite file per indexed commit, written with the workspace Diesel
dependency. Entries are write-once; readers open the file query-only. The schema
records the commit identity, producing run/attempt, schema version, and the
execution signature already used for validation compatibility (toolchain,
features, target, profile, coverage target). Coverage rows store compressed line
intervals per file per test with an index on (commit, file, interval) so a changed
range resolves to distinct tests in one indexed query. Interval compression keeps
the full-workspace graph in the single-digit megabyte range, compatible with
artifact retention.

A schema version mismatch, a malformed file, or a signature mismatch renders the
graph unavailable with a reason; it is never an error and never authorizes
excluding tests.

## Distribution and trust

The graph rides the existing artifact pipeline: successful default-branch coverage
runs publish it next to their validation records, and consumers locate it through
the same lazy, bounded, ancestry-ordered acquisition already used for baseline
records. Only the default-branch workflow writes; PR runs and local executions
never publish. Local planning downloads through the same authenticated GitHub
access the Flow already uses. No hosted database, release mirror, or new
credential is introduced. Artifact retention is acceptable because every main run
regenerates the graph and only the latest compatible ancestor matters.

## Query and amplification

Planning resolves the nearest compatible default-branch ancestor graph of the
comparison base. Changed line ranges on the base side query the graph directly.
Three conservative rules preserve soundness:

- Files changed between the graph commit and the actual comparison base widen
  those files to package scope, since the graph predates them.
- Changed ranges with no covering test, and files absent from the graph without
  an existing non-executable classification, widen to package scope.
- Unknown inputs, analyzer failures, and graph unavailability retain the existing
  full-workspace or package fallbacks with recorded reasons.

The result composes with the existing tiers: test-body isolation first, then
graph-derived exact selectors, then package plus reverse-dependency closure, then
workspace fallback. The plan records graph provenance and per-scope widening
reasons. Graph-derived selectors flow through the existing exact-filter consumers
(`run-tests.py`, `coverage_selection.py`); identity mismatches against the
executable inventory widen to the recorded baseline as they do today.

## Verification and review

Extend existing planning and Flow fixtures with synthetic graphs rather than real
instrumentation. Cover ancestor resolution order, interval queries, every widening
rule, schema/signature rejection, plan provenance, empty selections, and consumer
widen-on-mismatch. Ablate the amplification rules and provenance fields. Run the
spike on main before enabling PR consumption; compare selection size and job
duration against package-level selection on representative historical PRs. Keep
reports under target/, synchronize AGENTS.md and docs/ci.md, and archive the
completed specification.
