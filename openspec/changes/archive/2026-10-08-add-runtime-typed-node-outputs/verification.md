# Validation

- Simplified runtime and core-node regression suites passed, including public API examples and actual input/output derive compilation cases.
- The focused coverage suite passed 88 tests. A clean coverage build reports 85/85 covered lines in the output codec module without stale source mappings.
- All repository hooks passed: formatting, Clippy, cargo check, rust-analyzer, Markdown/YAML/TOML checks, workflow linting, and license auditing.
- Native macOS `nix flake check -L` passed all checks; the complete isolated suite passed 463 tests without skips. Linux checks are deferred to pull request CI.
- The change passed strict OpenSpec validation. The baseline repository has 11 existing strict-validation findings for long requirement bodies; synchronization must introduce no additional findings.
- Detailed ablation and coverage artifacts remain local under ignored `target/`.

## Archive validation

The reviewed change is archived with all implementation tasks complete. All 19 synchronized specifications pass ordinary validation. Strict findings match the pre-change baseline exactly; no warnings or errors were introduced. This change's completed tasks pass the archive audit. The existing `2026-10-05-unify-flow-runtime` archive has 17 unfinished tasks, unchanged by this work.
