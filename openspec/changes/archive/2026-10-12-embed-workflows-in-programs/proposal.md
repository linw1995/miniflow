# Embed Workflows in Rust Programs

## Why

[Issue #55](https://github.com/linw1995/miniflow/issues/55) requests a distribution mode that embeds
workflows through a macro in an existing program. Library users currently assemble definition parsing,
planning, and preparation themselves, while generated artifacts assume a standalone Cargo project.
The embedding API must validate providers during the host build, as `mf compile` does; an additional
`build.rs` configuration is acceptable.

## What Changes

- Add a build-time embedding helper to `mf-compiler`, behind its existing `codegen` feature.
- Add a declarative `embed_workflow!` macro that includes a named generated module in a Rust host.
- Reuse provider validation, immutable execution layouts, and runtime preparation.
- Parameterize generated artifact locations and anchor providers using host Cargo crate names.
- Generate a per-workflow manifest API and enforce its startup contract during preparation.
- Support multiple oneshot and streaming workflows in one host, with independent prepared instances.
- Document host-owned dependencies, execution resources, and incremental rebuild behavior.

## Capabilities

### New Capabilities

- `workflow-embedding`: build-validated workflows embedded as callable Rust modules.

### Modified Capabilities

- `workflow-binary-compilation`: share code generation with embedding while preserving standalone
  artifact paths, custom-runner fallback rules, and ordinal dependency aliases.

## Impact

Primary changes affect `mf-compiler` code generation and a new embedding module, Cargo host fixtures,
and embedding documentation. `mf-runtime` remains independent of the compiler. The host's Cargo
manifest and Cargo.lock own dependency resolution; embedding does not create a generated Cargo
project or a workflow-specific lock. No new package or procedural macro is required.
