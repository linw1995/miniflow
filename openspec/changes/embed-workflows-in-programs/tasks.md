# Implementation Tasks

## 1. Shared Generation

- [ ] Add artifact-location and entry-point parameters while preserving standalone defaults.
- [ ] Add minimal embedding artifact generation using shared validation and custom-runner fallback rules.
- [ ] Verify standalone output, execution, manifest inspection, and warning behavior remain compatible.

## 2. Build-Time Embedding

- [ ] Add `EmbedRequest`, typed errors, and the `codegen`-gated build helper at the compiler boundary.
- [ ] Validate names, provider mappings, duplicate output ownership, and definition paths.
- [ ] Generate validated artifacts under per-workflow `OUT_DIR` directories with Cargo input tracking.

## 3. Host Inclusion and Preparation

- [ ] Export a feature-independent declarative macro with renamed compiler dependency support.
- [ ] Anchor host provider crates and support explicit renamed support-crate bindings.
- [ ] Expose preparation and manifest APIs with applicable metadata and startup schema checks.
- [ ] Preserve independent instances and host ownership of execution options and resources.

## 4. Acceptance and Delivery

- [ ] Compile a host whose normal compiler dependency disables `codegen` and build dependency enables it.
- [ ] Cover oneshot, streaming, Loop/Iteration, startup inputs, observation, and typed providers.
- [ ] Preserve intermediate context outputs and the existing custom-runner typed fast-path fallback.
- [ ] Cover multiple workflows, renamed providers/support crates, and independent prepared state.
- [ ] Reject invalid graph/configuration/ports/providers during host builds with useful typed diagnostics.
- [ ] Verify definition/provider changes rebuild without cleaning and incompatible contracts fail before execution.
- [ ] Run a built host after removing its definition and generated source artifacts.
- [ ] Document Cargo setup, dependency ownership, module APIs, rebuilds, and manifest/TUI boundaries.
- [ ] Ensure fixture source inclusion in Nix and Cargo packaging.
- [ ] Run pinned focused checks, `prek install`, `prek -a`, `nix flake check -L`, and strict OpenSpec validation.
