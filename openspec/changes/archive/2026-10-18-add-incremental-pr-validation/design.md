# Design

## Context

See [proposal.md](proposal.md) for motivation and scope. CI currently checks out a tested PR merge commit and calls `scripts/affected_tests.py --commit`, which compares its first parent to the merged tree. The Python `--base` path and the opt-in `git.diff` task resolve a merge base. Neither operation expresses a direct comparison between successive tested merge snapshots.

Coverage preparation cleans previous profiles and workspace binaries, compiles selected packages together, runs package suites, and produces LCOV once. Partial reports currently use `rust-affected` for cumulative Codecov patch checks. Main/nightly use `rust` for full project coverage. Those PR coverage semantics must change with the comparison range.

## Goals / Non-Goals

**Goals:** Reuse prior successful validation, retain all newly affected consumers, make coverage use the same diff, preserve reproducible source identities, and fail or fall back explicitly when evidence is unavailable.

**Non-goals:** See the proposal. Retain package-level CI execution in the first delivery. A smaller increment can still select most packages when it changes a broadly consumed crate.

## Decisions

### 1. One success record per workflow attempt

Use a small JSON Actions artifact rather than a database or mutable branch-level marker. The CI helper reads run metadata and validation artifacts; selection logic remains local and receives explicit revisions.

The record contains a format/policy version, repository and PR identity, workflow run ID and attempt, PR head SHA, actual target-base SHA, tested merge commit/tree identities, predecessor record identity when reused, execution signature, selected scope, incremental coverage result/target, and explicit empty-coverage disposition where applicable. Store metadata, not old binaries or coverage profiles, for reuse.

A final validation job depends on the required lint, build, test, and incremental coverage checks. It publishes a record only after their success. A future run additionally requires the producing workflow attempt to have completed successfully; an artifact uploaded before a later job/post-step failure is insufficient. Reruns must not mix artifacts from different attempts. Empty selections can produce records only after all applicable checks pass.

### 2. Choose the nearest eligible PR-head ancestor

Candidates must belong to the same repository/PR and validation producer, identify the same actual target-base commit, use a compatible execution signature, and have a PR head that is an ancestor of the current PR head. Select the nearest eligible ancestor in commit history, not the run that finished most recently. Same-head retries can reuse a compatible successful validation without reclassifying unchanged source.

The execution signature includes validation-policy version, target, features, profile, effective toolchain/build settings, and the incremental coverage target. Do not hash the entire repository or every configuration file: the existing root-dependency and shared-configuration deferrals remain intentional. A compatibility reset selects the cumulative PR scope; it does not itself force workspace-wide execution.

Failed, cancelled, pending, foreign, stale, or malformed candidates are not reusable. Missing/expired artifacts, bounded-search exhaustion, unavailable API/history, or an inaccessible old merge object fall back to the cumulative PR diff with a diagnostic. An unusable current checkout or missing current comparison identity is an error, not an empty plan. Explicit CI orchestration may obtain required Git objects; the selector and Git node retain their no-fetch contract.

### 3. Directly compare two tested merge trees

For a reusable record, compare `previous_tested_merge` to `current_tested_merge` directly. Both merges must have the same actual base parent; their PR heads establish ancestry. The merge commits themselves need not be ancestors.

For initial or reset validation, compare the current merge's actual first parent to the current merge. Never use a stale base SHA from the event payload. All analysis, compilation, and execution use the current merged tree.

Add an explicit snapshot comparison mode to the existing Python selector and Git task. Existing default merge-base behavior stays compatible. Direct mode reports the supplied resolved base unchanged and handles old/new paths and ranges; it does not infer CI success. Forward the comparison mode and resolved identities into plan provenance. Git remains factual, LSP remains language-neutral, and CI baseline eligibility stays outside nodes.

Using only the event's `before` commit would miss an earlier failed/cancelled increment. Using `merge-base` between temporary merge commits would often select the target branch and repeat the entire PR.

### 4. Keep impact selection separate from validation reuse

Apply existing package ownership and all declared local reverse dependencies to the incremental files. Known shared executables seed their consumers. Deferred root dependency/configuration paths remain visible as ignored inputs. Unknown inputs and analyzer failures retain workspace-wide fallback.

Successful tests are not globally cached by name: if a package is affected again, run its suite again. Preserve empty plans without an unfiltered nextest call. Keep cumulative-PR fallback distinct from full-workspace fallback in summaries.

### 5. Gate the incremental diff locally

Execute selected tests with coverage once. Reuse the existing preparation and per-package phases and compute a local coverage result from the exact selected comparison and the current LCOV executable-line mappings. The result records source/configuration identity, changed executable line counts, covered/missed lines, target, and any unclassified paths.

Use one explicit numeric incremental target supplied by workflow policy and recorded in the
execution signature. At migration, resolve and document the effective existing PR coverage
requirement and choose a target no lower than that value; do not silently use a smaller threshold or
independently configurable defaults. Once established, runs need no live Codecov request to decide
the target. Threshold changes require compatible-record reevaluation. Main/nightly project coverage
retains its existing policy.

A missing/malformed report, wrong source identity, or unsupported mapping cannot establish a
successful coverage check. Absence of an LCOV entry alone does not prove that a changed Rust file
has no executable lines. Require independently supported non-executable classification, or fail with
a diagnostic and offer cumulative-scope rerun. Validate new files, renamed/deleted paths, zero-hit
executable lines, and overlapping/duplicate LCOV records. Deletion-only changes still select
affected tests even when they add no current executable lines.

Known deferred-only changes skip Rust coverage explicitly. A tested increment with a confirmed zero executable-line denominator is reported as not applicable, not 100% coverage. No iterative retry/widening loop is introduced. An explicit cumulative-PR rerun is available for diagnosis; it uses the same current merge snapshot and the cumulative diff for both selection and coverage.

### 6. Separate incremental artifacts from full Codecov reports

PR runs retain current LCOV, incremental coverage results, and the selection manifest as Actions artifacts and summaries. They do not upload incremental LCOV under a cumulative PR patch gate. Remove `rust-affected` from cumulative patch evaluation and replace the required PR gate with the final incremental validation check. Disable the obsolete cumulative PR patch status during migration; do not create a second contradictory gate even if an incremental run happens to execute all packages.

Main/nightly continue to upload complete reports under `rust`, with automatic discovery and carry-forward disabled. Missing expected reports or failed artifact publication fail the applicable job and prevent baseline advancement. Empty plans have an explicit exemption, not an accidental missing-report success. Do not merge prior LCOV data or claim that inherited successful validation is current cumulative coverage.

### 7. Keep orchestration small

Use a focused Python CI helper for record discovery/validation and incremental coverage evaluation, with only the separation needed for testability. Reuse existing Git/selection utilities where possible. Do not create a generic persistence backend, adapter registry, policy framework, or new node. Extend the existing timing summary with comparison origin, reused run, fallback reason, selected scope, and coverage range; do not present skipped work as zero-duration test samples.

## Risks / Trade-offs

- Changes can affect tests outside declared dependencies. Preserve existing conservative rules; main/nightly remain the accepted full-suite backstop.
- The required PR coverage status changes meaning and name. Migrate required-check settings with the workflow; a missing new status must not permit merging, and the retired status must not leave PRs permanently pending.
- Force pushes, target-base advances, policy changes, or artifact loss reduce reuse. Recompute cumulative PR affected scope instead of reconstructing unverifiable history.
- Missing executable mappings can block legitimate source-only edits. Test these cases before rollout and surface uncertainty; do not invent coverage or automatically exclude the file.
- Validation reuse does not prove unaffected behavior or preserve historical coverage counts. Reports must label inherited validation and currently measured coverage separately.

## Migration Plan

1. Land record resolution and explicit snapshot comparison with fixture tests while retaining the current workflow.
2. Land the local incremental coverage evaluator and document the explicit target migration before changing gating.
3. Wire selection, final validation records, summaries, and an explicit cumulative-scope rerun into CI. Keep main/nightly full.
4. Coordinate the required-check change with Codecov configuration and confirm the new status on a real PR before retiring the old requirement. Publish success records only under the new policy version.
5. Replay fixed revisions and successive pushes for PR #143 and the current tool PR. Measure selection, analysis, build, execution, and coverage independently; keep reports under ignored `target/`.
6. Roll back by disabling record reuse, restoring cumulative-PR selection and its matching patch-coverage gate together, and invalidating the new records. Do not revert only one side of the comparison/coverage contract.
