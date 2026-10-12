# Implementation Tasks

## 1. Shared Generation

- [x] Add artifact-location and entry-point parameters while preserving standalone defaults.
- [x] Add minimal embedding artifact generation using shared validation and custom-runner fallback rules.
- [x] Verify standalone output, execution, manifest inspection, and warning behavior remain compatible.

## 2. Build-Time Embedding

- [x] Add `EmbedRequest`, typed errors, and the `codegen`-gated build helper at the compiler boundary.
- [x] Validate names, provider mappings, duplicate output ownership, and definition paths.
- [x] Generate validated artifacts under per-workflow `OUT_DIR` directories with Cargo input tracking.

## 3. Host Inclusion and Preparation

- [x] Export a feature-independent declarative macro with renamed compiler dependency support.
- [x] Anchor host provider crates and support renamed support dependencies through hygienic imports.
- [x] Expose preparation and manifest APIs with applicable metadata and startup schema checks.
- [x] Preserve independent instances and host ownership of execution options and resources.

## 4. Acceptance and Delivery

- [x] Compile a host whose normal compiler dependency disables `codegen` and build dependency enables it.
- [x] Cover oneshot, streaming, Loop/Iteration, startup inputs, observation, and typed providers.
- [x] Preserve intermediate context outputs and the existing custom-runner typed fast-path fallback.
- [x] Cover multiple workflows, renamed providers/support crates, and independent prepared state.
- [x] Reject invalid graph/configuration/ports/providers during host builds with useful typed diagnostics.
- [x] Verify definition/provider changes rebuild without cleaning and incompatible contracts fail before execution.
- [x] Run a built host after removing its definition and generated source artifacts.
- [x] Document Cargo setup, dependency ownership, module APIs, rebuilds, and manifest/TUI boundaries.
- [x] Ensure fixture source inclusion in Nix and Cargo packaging.
- [x] Run pinned focused checks, `prek install`, `prek -a`, `nix flake check -L`, and strict OpenSpec validation.

## 5. Review and Archival

- [x] Review module boundaries, public APIs, typed error chains, and commits against AGENTS.md.
- [x] Run positive and negative ablations; remove unused artifacts, test-only abstraction, and duplicate coverage.
- [x] Validate the reviewed implementation and commit the simplification.
- [x] Archive the completed change and reconcile its capability specifications.
