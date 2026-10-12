## Why

Nightly baseline publication hashes generated Cargo manifests after collection, while CI restoration hashes a clean checkout. The incompatible keys prevent reuse. Documentation changes also force full tests, and main-branch CI does not refresh mappings between nightly runs.

## What Changes

- Hash source manifests and pinned build inputs consistently, excluding generated directories.
- Refresh compatible mappings incrementally on main, publishing only successful collections.
- Exempt known documentation from full-suite fallback while retaining conservative input guards.
- Document local affected-test iterations and reuse the existing Rust coverage fixture for regression checks.

## Capabilities

### New Capabilities

- `ci-test-selection`: reuse and refresh per-test coverage mappings with conservative fallbacks and separate complete coverage reporting.

## Impact

Changes affect CI workflows, coverage scripts, their existing tests, AGENTS.md, and CI documentation. Rust APIs, dependencies, and runtime behavior remain unchanged. Detailed ablation records stay under ignored `target/ablation/affected-baseline-refresh/`.
