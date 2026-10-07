## 1. Shared manifest contract

- [x] 1.1 Add the versioned manifest, fixed byte framing, shared validators, and typed Snafu errors in `mf-runtime`, following its existing module/export boundaries; verify round trips retain existing graph/interface versions and exact opaque node/port identities.
- [x] 1.2 Enforce the combined 16 MiB payload limit, bounded zero padding, checked lengths, supported versions, unique JSON members, and graph/interface consistency; verify malformed, duplicate, truncated, oversized, and mismatched records are rejected with retained typed causes.
- [x] 1.3 Document the framing and metadata exclusions in `docs/observability.md`; verify its field layout and version values match the format fixtures.

## 2. Coordinated build-time generation

- [ ] 2.1 Extend `mf-compiler/src/plan.rs` to return execution-plan source and manifest bytes from one validated preparation, covering task and stream paths; verify an external-provider fixture observes no additional factory construction for manifest generation.
- [ ] 2.2 Derive the startup schema from those same prepared nodes, including configuration-dependent ports and conditional stdin; verify generated records match dynamically prepared schemas and older workflow versions retain their empty startup interface.
- [ ] 2.3 Update the generated build script and project-layout/cache identity in `mf-compiler`; verify cold and warm builds regenerate both outputs after configuration/provider changes and preserve the prior executable on invalid preparation.
- [ ] 2.4 Update `docs/compiling.md` for manifest generation during the existing Cargo build; verify a generated project performs one runner build and needs no sidecar at inspection time.

## 3. Retained executable data and compatibility commands

- [ ] 3.1 Generate a fixed byte array in the ELF or Mach-O manifest section using target attributes and a live runner reference; verify native Linux and macOS release/LTO/strip fixtures retain exactly one readable record, including telemetry-disabled runners.
- [ ] 3.2 Serve `--describe` and `--describe-interface` from the embedded records without registry preparation; verify output preserves the existing JSON protocols and noisy or failing runtime factories are not called during inspection.
- [ ] 3.3 Document the preserved flags and frozen inspection behavior; verify command output agrees with the manifest decoded directly from the same built executable.

## 4. Runtime interface agreement

- [ ] 4.1 Compare freshly prepared task and stream startup schemas against the embedded schema during generated `--validate` and execution, without a second preparation or graph reconstruction; verify matching runners retain generated/in-memory execution parity.
- [ ] 4.2 Return typed node/port/resource mismatch errors before dispatch, source reads, timers, or workers; verify intentional provider-drift fixtures cover port membership, type, required flag, and stdin-condition differences and validation failure preserves the installed executable.
- [ ] 4.3 Document stable configuration-derived provider declarations and host/target agreement in the relevant provider/compilation documentation; verify examples distinguish interface declarations from runtime executor initialization and in-memory APIs remain independent of manifests.

## 5. TUI file-based preflight

- [ ] 5.1 Add narrowly configured `object` read support only to `mf-tui` and implement bounded regular-file/header/table/section reads with typed causes; verify ELF/Mach-O fixtures, large executables, invalid offsets, duplicate sections, and unsupported formats without whole-file unbounded allocation.
- [ ] 5.2 Integrate the manifest into `prepare_launch` while preserving argument transport, conditional stdin checks, observation-version checks, and receiver-before-child ordering; verify metadata preflight spawns no process and an invalid argument or active stdin requirement prevents execution.
- [ ] 5.3 Restrict bounded legacy command dispatch to a successfully recognized executable missing its manifest section; verify supported legacy finite/parameterized runners still work and corrupted or unsupported manifests never trigger fallback.
- [ ] 5.4 Update executable fixtures and legacy command-helper tests so script doubles do not masquerade as supported binary containers; verify cross-architecture inspection succeeds independently of execution and legacy timeout/output/exit/JSON failures retain existing behavior.
- [ ] 5.5 Update TUI/observability documentation and dependency/license notices as required; verify documented fallback conditions match tests and generated runners remain free of object parsing and terminal dependencies.

## 6. Integration validation

- [ ] 6.1 Exercise standalone inspection after moving task, stream, and nested-body runners away from build inputs; verify direct records, compatibility commands, validation, arguments, and observed execution agree for the same artifact.
- [ ] 6.2 Run `openspec validate embed-workflow-manifest --strict`, `nix develop --command prek install`, `nix develop --command prek -a`, and `nix flake check -L`; resolve failures and verify the change is ready for implementation review.
- [ ] 6.3 Review release-retention results on supported Linux and macOS targets and confirm every delta scenario has meaningful coverage; verify no behavior change is left undocumented or untested before completing implementation tasks.
