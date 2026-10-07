# Tasks

## 1. Runtime input contracts and codecs

- [ ] 1.1 Add `NodeInputs`, present-value and top-level field codecs, and Snafu-derived input decode errors through the existing runtime module entry point; verify missing/unknown field errors and inspectable typed sources in focused tests.
- [ ] 1.2 Implement strict bool, i64, f64, String, ValueRef, Vec, string-keyed BTreeMap, and top-level Option codecs; verify descriptor/decoder agreement, type aliases, numeric representation, omission/null distinctions, and recursive depth limits.
- [ ] 1.3 Decode shared values without an intermediate JSON tree; verify payload identity for raw fields and collection descendants, along with nested type mismatch paths and escaped port names.
- [ ] 1.4 Document codec invariants and supported owned types in public API documentation; verify examples against the runtime test suite.

## 2. Input derive macro

- [ ] 2.1 Add `mf-runtime-derive`, workspace dependencies, lockfile updates, and the `mf-runtime` re-export without a reverse runtime dependency; verify both packages build with the pinned toolchain.
- [ ] 2.2 Generate declarations and runtime-helper decoding for named-field structs, empty structs, generic bounds, raw identifiers, explicit field renames, and an explicit runtime path override; verify compile-pass fixtures including a renamed runtime dependency.
- [ ] 2.3 Reject unsupported shapes/types, borrowed fields, nullable collection elements, nested Option fields, malformed attributes, and empty/duplicate port names; verify compile-fail fixtures provide actionable diagnostics.
- [ ] 2.4 Document derive attributes and the boundary with Serde in `docs/node-development.md`; verify the documented input struct compiles and uses only runtime-owned conversion helpers.

## 3. Typed task preparation and execution

- [ ] 3.1 Add `TypedTaskNode`, a private task adapter, and source-preserving decode conversion into `NodeExecutionError`; verify typed business invocation and decode failures with node attribution and original sources.
- [ ] 3.2 Add fallible `PreparedNode::typed_task` using struct-derived inputs; verify rejection of competing input metadata, preservation of other metadata fields, and no execution or invocation decoding during preparation.
- [ ] 3.3 Exercise the adapter through synchronous execution and stream task domains; verify contexts, skip/missing precedence, pre-invocation type checks, source-bearing business failures, and output validation/publication behavior.
- [ ] 3.4 Document typed factory construction and execution alongside the existing dynamic API; verify both documented styles compile without changing task, event, or stream registrations.

## 4. Reference provider and compiler integration

- [ ] 4.1 Migrate `builtin.identity` to a derived ValueRef input struct and typed task preparation; verify its Any declaration, forwarding derivation, context publication, and shared payload identity remain intact.
- [ ] 4.2 Extend external-provider fixtures with typed scalar, recursive collection, optional, and renamed inputs; verify incompatible-edge rejection, typed startup arguments, no execution during validation/description, and generated versus in-memory behavior.
- [ ] 4.3 Verify derived startup declarations are frozen and checked by existing generated manifests without typed-provider special cases; cover matching declarations and drift rejection before dispatch.
- [ ] 4.4 Verify dynamic configuration-dependent providers and existing external task/event/stream fixtures remain supported; document the additive migration path and run their existing regression suites.

## 5. Integration validation

- [ ] 5.1 Run `nix develop --command bash scripts/run-cov.sh` for the new runtime and macro execution branches; inspect meaningful gaps and retain reports only under ignored `target/`.
- [ ] 5.2 Run `nix develop --command prek install`, `nix develop --command prek -a`, and `nix flake check -L`; resolve failures and verify all required repository checks complete successfully.
- [ ] 5.3 Run `openspec validate add-runtime-typed-node-inputs --strict --no-interactive` and review implementation against every added scenario; verify the change is ready for review with accurate task completion state.
