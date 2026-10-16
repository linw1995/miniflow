# Verification

- Installed both Git hooks with `nix develop --command prek install`.
- Ran `nix develop --command prek -a`: all hooks passed, including Clippy, Cargo check, rust-analyzer, Markdown,
  workflow validation, and license audit.
- Ran `nix develop --command env NEXTEST_PROFILE=ci python3 scripts/affected_tests.py --worktree --run`:
  649 tests passed, four skipped by existing configuration. The selector chose eight affected packages; the existing
  CI profile reused generated-runner caches without reducing the selected tests.
- Focused manifest and generated-metadata checks passed all eleven tests. Controlled behavior removals were rejected
  by regressions, and restored behavior passed. Detailed experiment records remain only under ignored `target/`.
- The change passed `openspec validate extract-runtime-inspection-contracts --strict --no-interactive`.
- Before archival, OpenSpec 1.14.2 strict validation reported four existing requirement-length warnings in
  `node-preparation` and twelve in `workflow-binary-compilation`, with no errors. These precede the new requirements.

The code and specification review found no unresolved implementation issues. Rust struct literals must initialize the
new fields; old serialized manifests remain compatible. No remote operation was requested or performed.

Archive validation synchronized three requirements into the two main specifications. Both main specifications pass
ordinary validation. Strict-mode issue lists are identical before and after archival: the same sixteen inherited
length warnings, no new warnings or errors. The archived task list is complete.
