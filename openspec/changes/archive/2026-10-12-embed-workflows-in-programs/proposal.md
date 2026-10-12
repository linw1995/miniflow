# Embed Workflows in Rust Programs

## Why

[Issue #55](https://github.com/linw1995/miniflow/issues/55) requests embedding workflows in existing
programs through a macro. Hosts should declare node crates only as normal dependencies. Generating
source during the host build and validating nodes during preparation keeps Cargo configuration
small and reuses the existing in-memory preparation APIs.

## What Changes

- Add a registry-free build helper that checks structure and emits named source modules.
- Include modules through a macro available without target-side compiler codegen.
- Prepare task and streaming workflows with the host registry, preserving typed errors.
- Expose structural plans and manifests derived from current host node metadata.
- Keep dependencies, observation, startup inputs, and execution resources under host control.
- Store fixed acceptance host sources and definitions as file fixtures.

## Capabilities

### New Capabilities

- `workflow-embedding`: build-time source generation and node validation during host preparation.

### Modified Capabilities

None. Standalone generation retains its existing build-time validation and artifact layout.

## Impact

Changes affect `mf-compiler`, acceptance fixtures, documentation, and an example host. The runtime
remains independent of the compiler. No procedural macro, provider binding table, frozen embedding
layout, separate dependency resolution, or nested Cargo build is introduced.
