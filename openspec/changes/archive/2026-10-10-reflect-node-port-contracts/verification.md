# Verification

- Pinned pre-commit and commit-message hooks installed with `nix develop --command prek install`.
- `nix develop --command prek -a`: all hooks passed on the final implementation.
- `nix flake check -L`: all native aarch64-darwin checks passed; 484 tests passed with none skipped.
- `nix develop --command cargo test -p mf-runtime --doc`: all five documentation tests passed.
- `nix develop --command bash scripts/run-cov.sh` with filters for runtime port contracts and typed tasks,
  compiler collection inference and typed plans, and both builtin provider packages: all 53 selected tests passed.
  The shared reflected-metadata helper covers all 13 executable lines. Reports are local under
  `target/coverage/result/`, including `lcov.info`.
- `openspec validate reflect-node-port-contracts --strict`: passed before archival; the final archived delta also
  passes against the original canonical specifications in an isolated local validation root.
- Post-archive canonical strict validation has exactly the same findings as `HEAD`: existing long-requirement
  warnings affect 11 of 19 specifications. This change introduces no additional canonical validation findings.

The implementation and specification review found no blocking issues. Detailed simplification and negative-control
records remain local under ignored `target/ports-ablation/` and are excluded from the commit.
