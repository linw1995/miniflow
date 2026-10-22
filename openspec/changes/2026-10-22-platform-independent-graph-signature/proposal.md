## Why

The coverage graph's execution signature binds the full `rustc -vV` output, which includes the host triple. Graphs are produced on Linux CI while local planning typically runs on macOS, so the signature would never match and local planning would always degrade to package-level selection. The line-to-test mapping does not depend on the host architecture, so the binding is over-conservative.

## What Changes

- The toolchain component of the graph execution signature becomes a platform-independent identity: the `release` and `commit-hash` fields of `rustc -vV`, rendered as `rustc <release> (<commit-hash>)`. Unparseable toolchain output is an explicit error, never a silent pass. The remaining signature fields (coverage-relevant environment, features, build target, nextest profile) are unchanged; the build target still participates because cross-compilation changes the produced binaries.
- This is safe across platforms: platform-specific tests absent from the consumer's runnable inventory widen to the recorded baseline through the existing identity-mismatch path, and platform-specific covered lines can only over-select, never skip, tests. PR coverage runs on one platform regardless, so relaxing the host binding introduces no CI regression.

## Capabilities

### Modified Capabilities

- `coverage-graph`: the execution signature binds a platform-independent toolchain identity instead of the full `rustc -vV` text.
