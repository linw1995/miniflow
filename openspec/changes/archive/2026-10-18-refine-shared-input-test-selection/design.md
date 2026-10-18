## Context

The current branch selected 716 cases because shared configuration forced full fallback. PR #143 selected 718. These measurements use fixed source snapshots and should be repeated without dropping changed files.

## Decisions

1. Package ownership has priority. Package manifests and fixtures select the owner and its reverse dependencies using every declared local dependency kind.
2. Explicit repository-level dependency, toolchain, nextest, CI/release configuration, and documentation paths do not expand PR Rust selection. Plans list them in `ignored_files`. Main and nightly retain full suites; non-Rust checks still run.
3. Known executable shared inputs seed concrete consumers: examples and generated-runner helpers reach CLI/compiler; release support reaches CLI; selector and Flow tooling reach the planner package. Missing all expected consumers falls back to the workspace.
4. Unknown paths, removed package sources, and analyzer failures retain full fallback. No semantic-edge absence proves exclusion.
5. Use ordinary path helpers in the two implementations and a differential policy regression. Do not add a policy registry, configuration language, node, or external policy artifact.
6. Add an optional-default `ignored_files` report field; the script continues to accept existing saved plans without it.

## Tradeoffs

Dependency and execution-configuration regressions may be detected only by main or nightly. This is an explicit coverage tradeoff, not a proof that ignored files cannot affect Rust behavior. Consumer mappings must evolve when new shared inputs are introduced; unknown inputs stay conservative.

The Flow still analyzes source snapshots before applying policy. A smaller selection does not establish faster analysis or measured test-runtime savings. PR #143 may remain broad because telemetry reaches runtime and its consumers.

## Validation

Exercise ownership priority, deferred-only empty plans, mapped consumers, missing consumers, unknown inputs, and analyzer failures. Compare Python baseline and Rust plans, then replay both pinned comparisons with the same test configuration. Keep experiments and ablation results under ignored `target/`. Run affected tests and repository hooks before review, archive, and submission.
