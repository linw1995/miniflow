## Why

The wrapper forces all tests for every fixture or example change even when cargo-affected can map that input through explicit metadata rules. The baseline cache also fingerprints standalone fixture manifests, preventing selective execution for fixture-only manifest changes.

## What Changes

- Map verified external inputs to their consuming test binaries through exact-path Cargo metadata rules.
- Reuse those exact paths for fallback decisions; retain full execution for unknown inputs and global build/test configuration.
- Restrict baseline cache identity to actual workspace manifests.
- Verify JSON inputs, uninstrumented Rust subprocess sources, incremental preservation, and conservative fallbacks with the existing real coverage fixture.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `ci-test-selection`: Allow verified external input rules without weakening unknown-input fallbacks or baseline compatibility.

## Impact

Changes Cargo metadata, CI/nightly cache expressions, the coverage selection helper, existing script tests, and developer documentation. PR coverage flags and nightly full verification remain unchanged. The new cache fingerprint requires an initial full baseline.
