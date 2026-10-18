## Why

Shared configuration currently turns small PRs into full Rust test runs. The repository accepts scheduled full verification as the backstop for dependency and orchestration changes.

## What Changes

- Resolve package ownership before shared-input policy, including manifests and fixture inputs.
- Defer known repository configuration and documentation to full main/nightly verification and independent checks.
- Map known shared executable inputs to their consumer packages and reverse dependencies.
- Keep unknown inputs and analyzer failures conservative, and expose ignored files in plans.
- Apply the same policy to the existing CI selector and the opt-in Flow planner.

## Capabilities

### Modified Capabilities

- `test-affected`: bounded shared-input policy and observable deferred files.
- `ci-test-selection`: selective PR verification with scheduled full coverage.

## Impact

Changes affect the Python selector, Rust planner, plan consumer compatibility, documentation, and focused policy regressions. No new nodes, rule engine, workflow scheduling, or release surface are introduced.
