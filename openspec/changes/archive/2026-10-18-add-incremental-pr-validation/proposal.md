# Proposal

## Why

Every PR push currently repeats selection over the complete PR diff, including changes whose tests and coverage already passed. Reusing the nearest compatible successful validation allows subsequent pushes to test only newly affected scope while preserving an explicit verification chain.

## What Changes

- Select a baseline from a completed successful run for the same PR, with compatible execution policy, the same actual target-base commit, and an ancestor PR head.
- Compare the previously tested merge snapshot directly with the current tested merge snapshot. Use the whole PR affected scope when no compatible record is available.
- Retain the existing ownership, shared-input exceptions, full reverse-dependency closure, and conservative analyzer fallback rules.
- Check coverage against the same incremental diff and advance the validation baseline only after every required check succeeds.
- **BREAKING**: Replace cumulative PR `codecov/patch` gating with a named incremental validation check. Keep full main/nightly project coverage and make the required-check migration explicit.
- Extend the opt-in Flow's existing Git task with an explicit snapshot comparison mode; CI continues to use the Python selector.

## Capabilities

### Modified Capabilities

- `ci-test-selection`: successful validation records, incremental selection and coverage, workflow completion, and coverage scope reporting.
- `test-affected`: direct snapshot comparison alongside the existing merge-base comparison, with comparison provenance in plans.

## Impact

Affected areas are the CI workflow, Python selection and CI helpers, coverage script/report handling, Codecov configuration, the existing development Git node, documentation, and focused regressions. No published package or release surface changes.

## Non-goals

No dependency-hop cap, recursive symbol-to-test graph, per-test execution cache, coverage carry-forward, cross-commit LCOV merge, automatic widening loop, additional Flow node, or mandatory LSP Flow adoption in CI. Main/nightly retain full verification.
