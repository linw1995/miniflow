## Validation

- All 31 script tests passed using the pinned development shell.
- The existing real coverage fixture verified selective JSON and uninstrumented Rust subprocess inputs, native rule attribution, nonempty reports, and preserved untouched mappings.
- All 15 new rules resolved against the actual nextest inventory, covering 35 exact input paths. Existing examples selected 2 to 33 tests; the shared fixture retained 283 CLI/compiler integration tests.
- Full coverage passed all 596 tests with two skipped and produced the standard reports, including LCOV.
- Repository hooks passed, including the new specification documents.
- `nix flake check -L` passed on aarch64-darwin, including all 596 nextest tests with two skipped.

## Ablation and review

Removing input exemptions, exact-path protection, global configuration protection, unknown example Rust protection, consumer selection, or incremental refresh broke the focused tests. Recursive fixture manifest hashing broke the actual Actions cache identity contract. Removing the now-redundant target exclusion retained it, so that exclusion was removed. Detailed experiment artifacts remain under ignored `target/ablation/refine-affected-inputs/`.

The inventory check found that nonexistent exact embedding binary IDs invalidate nextest expressions. Grouped name expressions keep existing consumers valid while including the embedding consumer visible in the open embedding PR. Unknown fixture inputs retain full-suite fallback.

The initial concurrent full validations exhausted disk space. Rebuildable workspace compilation artifacts and the task-owned failed coverage cache were removed; serial retries with a persistent fixture cache passed. No Rust implementation changes were needed.

PR patch coverage and nightly complete coverage keep their existing roles. The new root manifest and fingerprint require a fresh successful default-branch baseline before rebased PRs can restore it. Global nextest and workflow configuration still require the full suite.
