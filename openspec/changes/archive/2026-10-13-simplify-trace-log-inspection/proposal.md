## Why

Implementation review found stale empty-trace details at workflow completion and redundant mutable-preview invalidation. Controlled local ablations are needed to simplify the implementation while preserving the existing trace, log, and data contracts.

## What Changes

- Derive mutable Data and empty Details directly from the current observation snapshot.
- Remove the global data revision and rely on the pinned logging SDK's native context association.
- Keep immutable snapshot/preview caches and invocation-safe snapshot assembly.
- Consolidate overlapping preview tests, remove cache-identity assertions, and strengthen completion-transition and resource-fingerprint coverage.

## Capabilities

No capability requirements change. This fixes and simplifies behavior already specified by `workflow-terminal-ui` and `workflow-observability`; `skip_specs: true` preserves those contracts.

## Impact

Changes affect TUI preview refresh, snapshot state, the exporter, and focused tests. Ablation scripts, measurements, and mutation logs remain under Git-ignored `target/ablation/trace-log-review/`.
