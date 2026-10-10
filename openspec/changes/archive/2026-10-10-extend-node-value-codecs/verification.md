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

The reviewed implementation was committed as `4fb930b` before archival.

## Archive validation

The archive synchronized five added and four modified requirements. All nineteen main specifications pass ordinary
validation. Strict main-spec findings match the pre-change baseline exactly, with no new warnings or errors. This
change passes the completed-task audit; the unrelated historical unify-flow-runtime archive retains its existing
unfinished tasks.
