# Completion review

## Decision

No remaining blocking implementation gaps were found after resolving the two findings below. All 15 requirements map to implemented behavior and verification evidence, and all 30 implementation tasks are complete. The change is ready to archive.

## Scope

Reviewed the 15 requirements and 51 scenarios in both spec deltas against the implementation, regression tests, packaged acceptance, and all 30 implementation tasks. Source and test paths below are relative to `crates/`; bare test filenames refer to `mf-compiler/tests/` unless stated otherwise.

## Findings addressed before archive

### Generated directories could redirect writes outside the build

A cached `src` symlink was accepted, allowing generated source to replace an input file outside the managed directory. The failing regression reproduced this before the fix. Project synchronization now rejects symbolic links at both `src` and `target` before writing generated files or invoking Cargo.

Evidence: `mf-compiler/tests/dependency_project.rs` and `mf-cli/tests/compile.rs` verify that redirected directories are rejected and the original definition, existing executable, and dependency lock state remain intact.

### Working locks could share writes with authoritative state

The unchanged-content fast path retained symbolic links and Unix hard links. A subsequent write through a working lock could therefore modify the authoritative Flow lock before a successful build. The failing regression reproduced this before the fix. Synchronization now materializes independent files, and symbolic guard files are rejected before opening their targets.

Evidence: `mf-compiler/tests/build_state.rs` covers symbolic and hard-linked working files, guard aliases, process-exit lock release, and atomic replacement. The existing warm-build regression confirms unchanged private files still retain their timestamps.

## Requirement traceability

| Capability and requirement | Implementation | Evidence |
| --- | --- | --- |
| Dependencies: declarations | `mf-runtime/src/definition.rs` | `mf-compiler/tests/workflow_definition.rs`; packaged acceptance |
| Dependencies: deterministic input paths | `mf-compiler/src/inputs.rs` | `mf-compiler/tests/build_inputs.rs`; same-directory Flow acceptance |
| Dependencies: kinds and registration | `mf-compiler/src/dependency_project.rs`; runtime registry | Multi-kind fixture; `runner_validation.rs`; no implicit registry test |
| Dependencies: resolution locking | `cargo_build.rs`; `pipeline.rs`; `state.rs` in `mf-compiler/src/` | `dependency_resolution.rs`; failure preservation and locked rebuild tests |
| Dependencies: runtime compatibility | `mf-compiler/src/compatibility.rs` | `mf-compiler/tests/runtime_compatibility.rs`; real resolved fixture graph |
| Dependencies: project state protection | `inputs.rs`; `state.rs`; `pipeline.rs` in `mf-compiler/src/` | Input alias tests; build-state tests; CLI failure and redirection tests |
| Compilation: standalone executable | `mf-compiler/src/pipeline.rs` and generated runner | `runner_project.rs`; `scripts/test-packaged-cli.py` at repository root |
| Compilation: project registry | `mf-compiler/src/dependency_project.rs` | Unknown and duplicate kind, configuration, and multi-kind tests |
| Compilation: graph and port validation | `mf-compiler/src/compiler.rs` | `structural_planning.rs`; `workflow_validation.rs`; `workflow_planning.rs` |
| Compilation: build diagnostics | `mf-compiler/src/pipeline.rs` and `cargo_build.rs` | Cargo failure, missing executable, invalid configuration, and installation failure tests |
| Compilation: schema migration | `mf-runtime/src/definition.rs` | Old-schema migration, unsupported version, and example round-trip tests |
| Compilation: build-directory reuse | `mf-compiler/src/cache.rs` and `dependency_project.rs` | Ownership, output-path independence, warm build, and removed-cache tests |
| Compilation: current-input validation | Generated validation mode; Cargo invocation | Feature, source, version, flag, compiler identity, and partial-build recovery tests |
| Compilation: directory protection | `cache.rs`; `dependency_project.rs`; `state.rs` in `mf-compiler/src/` | Contention, canonical aliases, linked directories, and linked state regressions |
| Compilation: validate the installed binary | `mf-compiler/src/pipeline.rs` | Execution sentinel test; same-binary validation and standalone acceptance |

## Design and task alignment

The implementation uses one runner with validation and execution modes, explicit link anchors, embedded Flow dependencies, per-definition locks, and reusable owned build directories. The former task wording about a shared aggregation module was corrected to link anchors; a separate module is unnecessary with one binary target.

Packaged acceptance and release prerequisite checks are ordinary integration tests discovered by nextest, including coverage and Nix checks. Dependency preparation fetches the complete locked graph before offline fixtures run, covering cold Cargo caches as well as the local development cache.

## Delivery boundaries

Actual registry publication is a release prerequisite, not an unfinished implementation task. Acceptance verifies real package archives through an isolated Cargo registry source; it does not publish packages or claim that the public registry is ready for a release.

The CD ownership check and requirement for all four workspace packages are additional release policy. The CLI's direct support dependencies are runtime and compiler; built-in nodes remain explicitly selected Flow dependencies. Registry ownership is not a runtime compatibility mechanism and can be managed separately from the resolver.

Third-party build scripts and factories retain the documented native-code trust model. The directory and state checks protect CLI-managed build operations; they do not sandbox plugin code.

## Final validation

- OpenSpec strict validation passed.
- Full repository hooks and `nix flake check -L` passed on aarch64-darwin.
- The coverage workflow passed all 73 tests, including packaged acceptance through nextest.
- One no-subprocess filesystem test received a nextest leak warning during the instrumented run. Twenty isolated instrumented repetitions then passed without warnings; the full run had no test failures.
- Regression tests for both findings failed before the fixes and pass afterward. CLI coverage also verifies that redirected cache directories cannot modify the definition, executable, or dependency lock state.
