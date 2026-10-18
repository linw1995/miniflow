# Tasks

Implement and commit each numbered group in order with its focused tests and documentation. The planning artifacts are complete when validated; the implementation checkboxes remain open until the work actually passes verification.

## 1. Successful validation records and baseline resolution

- [ ] 1.1 Define the record format and execution compatibility signature in a focused CI helper; verify repository/PR identity, run attempt, policy, source, and coverage fields with fixture records and document the format in `docs/ci.md`.
- [ ] 1.2 Resolve the nearest successful ancestor PR head using completed workflow metadata and matching records; verify successful chains, failed/cancelled/pending intermediates, out-of-order completion, and same-head retries.
- [ ] 1.3 Implement bounded discovery and cumulative-PR fallback; verify force pushes, changed target bases, incompatible policy, expired/missing artifacts, unavailable Git objects/API, and malformed records preserve explicit reasons without authorizing an empty success.
- [ ] 1.4 Keep deferred dependency/configuration policy distinct from execution compatibility; verify compatibility resets select cumulative affected scope rather than automatically forcing all workspace packages.

## 2. Direct snapshot comparison and affected selection

- [ ] 2.1 Add explicit snapshot comparison to the Python selector without changing existing comparison defaults; verify sibling merge snapshots use their direct diff while ordinary branch comparison still uses a merge base.
- [ ] 2.2 Extend the existing Git task and plan provenance with the same comparison semantics; verify resolved identities/ranges, invalid modes, no-fetch behavior, and compatibility with saved plans in focused Rust and consumer regressions.
- [ ] 2.3 Apply existing ownership/shared-consumer rules and complete reverse closure to incremental changes; verify a previously passing suite reruns when newly affected and that ignored-only changes produce no nextest invocation.
- [ ] 2.4 Update selector usage and the tool README with initial, incremental, and reset examples; verify example plans agree between Python and the opt-in Flow for equivalent local snapshots.

## 3. Incremental coverage evaluation

- [ ] 3.1 Implement current-side changed executable-line coverage using the exact comparison and current LCOV; verify new/renamed/deleted files, zero-hit lines, duplicate records, and correct numerator/denominator on small compiled fixtures.
- [ ] 3.2 Implement explicit failure for missing, malformed, stale, or uncertain executable mappings and explicit not-applicable results for confirmed zero denominators; verify report absence cannot masquerade as a coverage pass.
- [ ] 3.3 Establish one explicit PR target no lower than the verified effective pre-migration requirement, record it in compatibility metadata, and document the migration; verify boundary percentages and target changes cannot silently weaken or reuse incompatible validation.
- [ ] 3.4 Integrate evaluation with the existing single coverage build and package execution phases; verify selected tests are not rerun solely to collect coverage and that test/report failures preserve unsuccessful status.

## 4. CI orchestration, reporting, and gate migration

- [ ] 4.1 Wire record discovery and explicit comparisons into PR CI while retaining the current merge checkout; verify recorded actual merge parents rather than stale event base metadata determine eligibility and reset scope.
- [ ] 4.2 Add the final validation-record job after all required checks; verify build/lint/test/cov failures, cancelled attempts, and artifact-publication failures cannot become reusable success, while explicit empty selections can advance after applicable checks pass.
- [ ] 4.3 Replace cumulative PR Codecov patch gating with the incremental validation check and scoped Actions artifacts; verify full main/nightly uploads retain `rust`, discovery/carry-forward remain disabled, and required-check settings transition without a missing gate or obsolete blocker.
- [ ] 4.4 Provide an explicit cumulative-PR rerun for a pinned current merge snapshot and update timing summaries; verify selection and coverage share the requested range and inherited validation is distinguished from measured coverage.
- [ ] 4.5 Update `AGENTS.md`, `docs/ci.md`, and workflow/coverage regressions for the final behavior; verify empty plans, missing expected reports, and unchanged main/nightly full-suite scheduling.

## 5. Integrated replay and ablation

- [ ] 5.1 Replay successive fixed revisions from PR #143 and the current tool PR, including successful, unsuccessful, and reset transitions; verify both chosen baselines and nextest case identities, with reports only under ignored `target/`.
- [ ] 5.2 Compare cumulative versus incremental selection, analysis cost, build cost, execution time, and coverage on matching head/configurations; verify any savings claim is tied to recorded measurements rather than case counts alone.
- [ ] 5.3 Ablate success eligibility, attempt identity, direct snapshot comparison, and missing-report guards; verify meaningful regressions fail, then remove duplicated tests and unnecessary abstraction while preserving those boundaries.
- [ ] 5.4 Run affected checks, relevant Python/Rust/coverage regressions, `prek -a`, and strict change validation; review the complete diff and record remaining limitations before archival.

## Workflow follow-up

- Commit the reviewed plan, then land implementation groups separately using the repository's commit conventions.
- After implementation and review, archive the change and validate synchronized specifications.
- Update the existing PR when implementation is ready, then monitor CI and review feedback through completion.
- Do not change repository required-check settings until the replacement check has been demonstrated; coordinate that rollout with the workflow and Codecov changes.
