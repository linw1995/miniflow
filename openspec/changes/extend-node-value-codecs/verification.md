# Verification

- Pinned pre-commit and commit-message hooks installed with `nix develop --command prek install`.
- `nix develop --command prek -a`: all hooks passed on the reduced implementation and specification.
- Focused runtime/derive/Code and real generated-runner regressions passed after production and test simplification.
- Reversible source controls failed at behavioral assertions as expected and were restored.
- `nix flake check -L`: all native aarch64-darwin checks passed; 494 tests passed with none skipped.
- `nix develop --command bash scripts/run-cov.sh` excluding unrelated crates: 102 affected tests passed.
  Coverage artifacts remain local under target/coverage/result/, including lcov.info.
- `openspec validate extend-node-value-codecs --strict --no-interactive`: passed before archival.

The final AGENTS.md review found no blocking issues. Detailed experiment records remain local under ignored
target/ablation/node-value-codecs/ and are excluded from commits. Linux checks were not executed on this native host.
