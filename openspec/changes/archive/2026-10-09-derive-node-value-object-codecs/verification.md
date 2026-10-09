# Verification

- Pinned pre-commit and commit-message hooks installed with `nix develop --command prek install`.
- `nix develop --command prek -a`: all hooks passed.
- `nix develop --command cargo test -p mf-runtime`: 77 tests passed, including five doctests.
- `nix flake check -L`: all native aarch64-darwin checks passed; 481 tests passed with none skipped.
- `nix develop --command bash scripts/run-cov.sh` with the runtime output and derive-compile binary filter:
  all ten selected tests passed. Both object conversion helpers cover all 41 executable lines. Reports are local under
  `target/coverage/result/`, including `lcov.info`.
- `openspec validate derive-node-value-object-codecs --strict`: passed.

The final review found no blocking issues. Experimental records remain local under ignored `target/` and are not part
of the committed specification.
