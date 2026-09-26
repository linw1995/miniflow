# Tasks

## 1. Remove production bundle coupling

- [x] 1.1 Remove the production compiler dependency on `mf-bundle` and migrate callers of `plugin_registry()` to explicit registries; verify the compiler production dependency graph contains no built-in node crate (keep a temporary explicit CLI bundle until task 4.2) and existing graph-validation tests pass.
- [x] 1.2 Add an external fixture crate registering multiple kinds through the existing runtime contract, without a `kind()` export; verify both registrations survive optimized linking through an explicit crate anchor.
- [x] 1.3 Document the registration, shared-runtime, and factory side-effect contracts in `docs/plugins.md`; verify the fixture conforms to the documented contract.

## 2. Unified Flow definition and CLI inputs

- [x] 2.1 Add required embedded dependencies in schema `2026-09-26` and source/feature validation; test registry, full-revision Git, local paths, aliases, missing dependencies, unknown fields, conflicting declarations, and migration diagnostics for `2026-09-24`.
- [x] 2.2 Add `--locked`, definition-relative dependency paths, and adjacent per-definition lock naming; test unrelated working directories, symlinked definitions, extensionless paths, and two Flows in one directory.
- [x] 2.3 Protect the definition and lock from executable output collisions; verify direct paths, derived lock collisions, and symlink aliases cannot overwrite inputs; verify successful compilation leaves the definition byte-for-byte unchanged.
- [x] 2.4 Document embedded dependency syntax, explicit built-ins, path behavior, and schema migration in `docs/workflows.md`; upgrade example JSON and verify definitions round-trip with their dependencies intact.

## 3. Generated package and dependency resolution

- [x] 3.1 Generate one stable package with one runner target with validation and execution modes, deterministic dependency aliases, and a shared plugin aggregation module; test renamed packages, multi-kind crates, and feature forwarding.
- [x] 3.2 Seed Cargo resolution from the per-definition lock file and implement `--locked` behavior; test first resolution, compatible reuse, required updates, missing locks, and incompatible locks using isolated dependency fixtures.
- [x] 3.3 Inspect the resolved runtime identities and report conflicting dependency paths; test mismatched runtime versions and sources before executing runner validation.
- [x] 3.4 Implement an OS-backed build lock keyed by the dependency lock path and atomic lock persistence after runner success; test contention, lock release after process exit, prior-lock preservation on failure, and persistence failure before executable installation.
- [x] 3.5 Document lock semantics, local-path mutability, and contention behavior; verify the documented repeated-build commands preserve the lock under `--locked`.

## 4. Single-runner validation and executable construction

- [x] 4.1 Separate structural planning from plugin validation and implement the runner validation mode without executing nodes; test unknown and duplicate kinds, invalid configuration, and factories printing diagnostics.
- [x] 4.2 Connect the CLI to one runner build followed by validation, remove its temporary bundle dependency, and require successful validation before installation; verify feature-sensitive fixture behavior agrees between validation and execution.
- [x] 4.3 Integrate stage-specific errors, expected-artifact checks, retained projects, lock persistence, and atomic executable replacement; inject validation exit failures, missing executables, Cargo failures, and installation failures and verify existing outputs survive.
- [x] 4.4 Update `docs/compiling.md` for the single-runner build lifecycle, toolchain requirements, diagnostics, and native-code trust model; verify retained-project instructions against an intentional fixture failure.

## 5. Reusable build directories

- [x] 5.1 Add stable default cache selection, `--build-dir`, ownership metadata, and created/reused diagnostics; test unchanged identity across output-path changes and rejection of foreign or incompatible explicit directories.
- [x] 5.2 Add build-directory locking after dependency-file locking and reject overlapping input/output paths; test concurrent reuse, process-exit lock release, independent Flow directories, and canonical aliases.
- [x] 5.3 Retain generated projects and Cargo artifacts, write only changed files, and resynchronize working locks from Flow locks; test generated-file modification times, Cargo freshness output, and authoritative lock restoration after failed resolution/build attempts.
- [x] 5.4 Synchronize complete generated artifact sets, remove obsolete generated configurations, and repair recognized partial state; test graph/config changes, removed nodes, validation interruption, and failure after a previous successful build without installing stale output.
- [x] 5.5 Verify invalidation through dependency, feature, lock, local source, toolchain, and compiler-flag changes; confirm affected outputs rebuild through Cargo and unchanged dependencies remain reusable without timing-based assertions.
- [x] 5.6 Document directory selection, reuse, retained local data, and deletion of inactive entries in `docs/compiling.md`; verify documented warm builds, explicit temporary directories, and clean rebuilds after directory deletion.

## 6. Support package distribution and migration

- [x] 6.1 Prepare exact-version runtime/compiler package dependencies and selectable built-in packages; verify packaged contents have no production bundle dependency or implicit checkout paths using Cargo package verification in an isolated registry.
- [x] 6.2 Make release CLI builds resolve support packages by exact version and provide an explicit internal source override for development tests; verify release behavior has no automatic checkout fallback.
- [x] 6.3 Add an isolated-registry acceptance fixture that uses packaged support and third-party crates with an installed CLI outside the checkout; verify it compiles and executes without the internal source override.
- [x] 6.4 Define and document the support-package-before-CLI release gate, including registry ownership and availability checks; verify release preparation fails clearly for unavailable support versions without publishing packages as part of tests.
- [x] 6.5 Remove the obsolete fixed bundle after migrating its tests and examples; update README and plugin instructions, then verify the documented built-in example and third-party example through the new CLI path.

## 7. End-to-end acceptance

- [ ] 7.1 Exercise registry, pinned local Git, and local-path fixtures from unrelated working directories, including two definitions in one directory with different node sets using the same CLI; verify outputs, isolation, and locked rebuild behavior.
- [ ] 7.2 Move a generated executable away from project files and run it without Cargo on PATH; verify selected outputs and existing deterministic DAG, cycle, port, and runtime-error behavior.
- [ ] 7.3 Run `openspec validate build-flows-with-third-party-nodes --strict`, install hooks with `nix develop --command prek install`, and run `nix develop --command prek -a` plus `nix flake check -L`; resolve failures before submitting the implementation.
