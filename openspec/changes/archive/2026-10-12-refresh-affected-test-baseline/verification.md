# Verification

- The simplified suite passed 30 Python tests, including the real Rust fixture's selection, mapping preservation, missing-data and fingerprint fallbacks, full and partial coverage, and failed-child handling.
- Repository hooks passed under the pinned development shell, and required Git hooks were installed.
- Native aarch64-darwin flake checks passed, reusing the successful 596-test Rust result with two skips for unchanged Rust sources.
- Full native coverage with the CI profile passed 596 tests with two skips and generated LCOV, Cobertura, and HTML reports under ignored target/coverage/result/.
- GitHub's glob implementation verified consistent restore/save inputs, generated-directory exclusion, and source-manifest invalidation.
- Isolated ablations confirmed the retained protections and the consolidated fixture's improved failure detection. Detailed reports, logs, variants, and source hashes remain under ignored target/ablation/affected-baseline-refresh/.
- The active change passed strict OpenSpec validation before archive.
- Archive synchronized the new main specification, which passed strict validation; this change passed the completed-task archive audit.

The global archive audit retains the existing 17 unfinished tasks in the unrelated 2026-10-05-unify-flow-runtime archive. This change introduces no additional audit findings.

GitHub cache publication and Linux execution remain subject to subsequent repository CI. This local change does not claim post-deployment validation.
