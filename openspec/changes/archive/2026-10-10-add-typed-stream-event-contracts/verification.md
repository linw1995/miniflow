# Verification

- Installed pinned hooks with `nix develop --command prek install`.
- `nix develop --command prek -a`: all hooks passed.
- `nix flake check -L`: all native aarch64-darwin checks passed; 486 tests passed with none skipped.
- `nix develop --command cargo test -p mf-runtime --doc`: all five documentation tests passed.
- `nix develop --command bash scripts/run-cov.sh` with runtime typed-event, port-contract, typed-task,
  compiler typed-task, and builtin core filters: all 30 selected tests passed. Typed event/stream conversion
  covers all 47 executable lines. Reports remain local under `target/coverage/result/`, including `lcov.info`.
- `openspec validate add-typed-stream-event-contracts --strict`: passed.

Final review found no blocking issues. Reversible simplification and negative-control records remain under ignored
`target/typed-stream-event-ablation/` and are excluded from the commit.
