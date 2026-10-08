# Validation

- Simplified runtime and core-node regression suites passed, including public API examples and actual input/output derive compilation cases.
- The focused coverage suite passed 88 tests. A clean coverage build reports 85/85 covered lines in the output codec module without stale source mappings.
- All repository hooks passed: formatting, Clippy, cargo check, rust-analyzer, Markdown/YAML/TOML checks, workflow linting, and license auditing.
- Native macOS `nix flake check -L` passed all checks; the complete isolated suite passed 463 tests without skips. Linux checks are deferred to pull request CI.
- The change passed strict OpenSpec validation. The baseline repository has 11 existing strict-validation findings for long requirement bodies; synchronization must introduce no additional findings.
- Detailed ablation and coverage artifacts remain local under ignored `target/`.
