# Tasks

## 1. Extend the shared type model

- [x] 1.1 Run an isolated CEL engine feasibility spike before changing runtime types; verify inferred scalar/list/map result types, distinguish explicit `dyn(...)` from macro-internal `Dyn`, and verify JSON conversion and execution on the pinned toolchain, recording the result under Git-ignored `target/`.
- [x] 1.2 Add `Int64`, `Float64`, recursive `List` and `Map` variants while retaining existing constructors and broad variants; verify static plugin registrations and configuration-derived typed ports compile against the updated runtime.
- [x] 1.3 Implement static, runtime-checked, and incompatible structural compatibility; verify a table of scalar, broad, `Any`, nested collection, and empty-collection cases rejects concrete member mismatches without coercion.
- [x] 1.4 Implement one recursive JSON value validator with deterministic type names and JSON Pointer paths; verify signed boundaries, floating representation, null, homogeneous nested values, escaped keys, and descriptor-depth errors.
- [x] 1.5 Document refined descriptors and the `ValueType` borrow/clone migration in `docs/plugins.md`; verify code examples compile with the updated public API.

## 2. Enforce declared types during execution

- [x] 2.1 Validate every produced output before context publication; verify wrong types fail even without consumers and no output from a failed node becomes visible.
- [x] 2.2 Validate bound inputs after dependency availability and skip resolution but before node execution; verify dynamically narrowed values, optional inputs, skipped targets, and missing-output precedence.
- [x] 2.3 Add shared runtime tests for nested path diagnostics and repeated runs; verify no partial or cross-run values survive a type failure.
- [x] 2.4 Document shared runtime input/output guards and stricter plugin output behavior in `docs/plugins.md`; verify a direct Flow with an `Any` source feeding a refined input succeeds for a matching value and fails for a mismatch.

## 3. Apply the model in compilation and generated binaries

- [x] 3.1 Replace exact-only compiler edge checking with the three-way compatibility result; verify accepted refined-to-broad and broad-to-refined edges plus rejected concrete and nested mismatches with both endpoints in diagnostics.
- [x] 3.2 Exercise the same typed workflow in `Flow::execute` and a generated runner; verify matching values, runtime-checked failures, skip precedence, and no kind-specific generated guards.
- [x] 3.3 Update the external plugin and packaged CLI fixtures with refined static and dynamic ports; verify unchanged broad-port plugins still build and matching runtime/compiler package identities remain required.
- [x] 3.4 Document the support-package and Rust API migration in plugin and release guides; verify the examples distinguish compatibility at build time from value checking at run time.

## 4. Integration checks

- [x] 4.1 Run `nix develop --command bash scripts/run-cov.sh` and inspect refined compatibility, recursive validation, and skip-precedence coverage; add cases only for uncovered behavior.
- [x] 4.2 Run `nix develop --command prek install`, `nix develop --command prek -a`, `nix flake check -L`, and `openspec validate extend-workflow-port-types --strict`; verify all required checks pass before applying the dependent CEL Code change.
