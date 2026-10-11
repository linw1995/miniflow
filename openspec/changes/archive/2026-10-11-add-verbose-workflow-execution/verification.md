# Verification

- Pinned pre-commit and commit-message hooks installed with `nix develop --command prek install`.
- Implementation and matching test mutations completed under the pinned toolchain and restored the selected source.
- Selected parser control: four tests passed. Direct CLI process control: five tests passed.
- Moved task, Loop, and stream runner acceptance passed with the reduced alias matrix.
- Generated runner OTLP acceptance passed with local verbose diagnostics on success and handled failure.
- Telemetry-disabled compile acceptance passed, including verbose compilation details.
- Repository coverage script: all 17 selected parser and CLI tests passed. Reports remain local under
  `target/coverage/result/`, including `lcov.info`.
- `openspec validate add-verbose-workflow-execution --strict`: passed.
- Existing main specifications: standard validation passed; strict validation records 25 existing length warnings
  without errors as the archival comparison baseline.

- `nix develop --command prek -a`: all hooks passed.
- `nix flake check -L`: all native aarch64-darwin checks passed; 536 tests passed, two skipped.

The final review found no blocking issues. Required code checks passed before archival. Experimental records remain
under ignored `target/` and are excluded from the implementation commit.

- Reviewed change archived and all six requirements synchronized into the three existing main specifications.
- Updated main specifications pass standard validation; strict findings match the 25 pre-existing warnings exactly.
- Archived-change validation: this change passes with no findings. The all-archive scan retains an existing failure
  in `2026-10-05-unify-flow-runtime` (17 incomplete tasks), which predates this implementation and remains untouched.
