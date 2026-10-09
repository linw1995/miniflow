## Why

The unified contract is available, but Constant, Batch, and Readline still duplicate fixed port declarations and manually assemble values. Identity uses the unified derive without advertising the generated execution contract, so ordinary builtin chains cannot use typed segments.

## What Changes

- Migrate Constant, Batch, and Readline fixed port bags to `NodeValue`, retaining configured refinements, event timing, stdin ownership, and shared payload identity.
- Advertise Identity's certified typed fields through a provider-owned exported constructor.
- Retain dynamic interfaces for IfElse, Loop, and Code, whose port sets depend on configuration.
- Extend existing builtin and generated-runner regressions instead of introducing parallel test harnesses.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `typed-port-contracts`: apply the unified contract to fixed builtin port bags and enable eligible Identity chains.

## Impact

Changes affect `mfn-core`, existing compiler integration tests, and node development documentation. No new dependencies or runtime APIs are required.
