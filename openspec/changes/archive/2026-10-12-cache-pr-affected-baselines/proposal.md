## Why

PR CI restores only main mappings and never refreshes its own coverage database, so later commits repeat the accumulated diff or the original full-suite fallback. Successful aggregate coverage cannot serve as a per-test baseline.

## What Changes

- Cache successful PR mappings separately by PR, environment fingerprint, and tested commit; restore the latest compatible PR snapshot before falling back to main.
- Preserve required Git commit objects for ephemeral PR merge refs without caching build directories.
- Collect mappings and PR patch coverage from one test execution by exporting compact merged counters.
- Apply fixture input rules during incremental collection as well as status/run.
- Delegate retention to GitHub's inactivity and least-recently-accessed eviction policies.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `ci-test-selection`: Reuse isolated successful PR baselines while preserving main ownership and patch feedback.

## Impact

Changes CI cache steps, the existing coverage scripts and real fixture, a small pinned cargo-affected patch, and developer guidance. Failed or cancelled collection cannot publish a PR snapshot. Main and nightly retain their baseline and complete coverage roles.
