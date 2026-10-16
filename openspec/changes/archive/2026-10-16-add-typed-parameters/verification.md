## Verification

- Pinned Nix toolchain and installed pre-commit/commit-msg hooks.
- 68 affected runtime/compiler tests passed, including parameter source precedence,
  input snapshots, context reuse, typed contracts, interface drift, stream startup,
  and generated runner argument/JSON/file transports and isolated execution.
- `prek -a` passed all formatting, Clippy, Cargo check, rust-analyzer, Markdown,
  workflow, and license audit checks.
- Both affected main specifications passed strict OpenSpec validation. The new archive
  passed the completed-task audit. All 28 main specifications passed ordinary validation.
- Repository-wide strict audits also reported existing overlong requirements and an
  unrelated archive, `2026-10-05-unify-flow-runtime`, with 17 incomplete tasks. This
  change leaves those historical artifacts unchanged.
- Detailed ablation scripts and evidence are local under Git-ignored `target/` and are
  excluded from the commit. The completion review records the retained design.
