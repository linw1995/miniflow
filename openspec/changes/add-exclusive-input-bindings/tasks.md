## 1. Definition and prepared binding contract

- [x] 1.1 Add default-empty exclusive groups to workflow, loop, and iteration definitions; verify round trips, unknown-field rejection, and unchanged serialization for existing definitions.
- [x] 1.2 Add explicit prepared input groups and equivalent programmatic construction support; verify duplicate sources, fewer than two sources, ordinary/group conflicts, unknown endpoints, and required-input connectivity.
- [x] 1.3 Include every candidate in dependency, cycle, scope, context-ancestry, worker visibility, and message-domain analysis; verify cycles and cross-domain references are rejected while legal scope sources remain accepted.
- [x] 1.4 Document the binding syntax and low-level Rust API additions in workflow and embedding documentation; verify examples parse through definition preparation.

## 2. Shared resolution and type evidence

- [x] 2.1 Resolve all candidate outcomes before consumer activation using shared values and typed Snafu errors; verify unique, all-skipped, duplicate-active, missing, false/null, equal-value ambiguity, and reordered candidates.
- [x] 2.2 Preserve AND gates, connected optional behavior, missing-before-ambiguity-before-skip precedence, and skipped-node type-check behavior; verify no consumer side effect occurs on a skipped or invalid binding.
- [x] 2.3 Validate each candidate and aggregate conservative type and exact-value evidence; verify inactive incompatible candidates, dynamic input failures, and forwarding derivations do not select arbitrary source facts.
- [x] 2.4 Preserve shared payload identity and secret metadata; verify identity forwarding and redacted snapshots, and document independent groups versus one structured correlated value.

## 3. Generated and scoped execution

- [x] 3.1 Emit exclusive groups in compiled plans and conservatively exclude affected ownership transfers with a reported typed fallback; verify in-memory and standalone parity and preservation of unrelated typed segments.
- [x] 3.2 Integrate logical input resolution with Task, Event, and Stream consumers; verify delayed competing candidates, failure/cancellation before resolution, and collecting-node buffer/deadline preservation on all-skipped inputs.
- [x] 3.3 Carry groups through loop rounds and sequential/parallel iteration preparation; verify changing winners and all-skipped outcomes never reuse prior frame or body values.
- [x] 3.4 Preserve isolated-node and embedded execution contracts; verify callers can supply a resolved input without invoking candidate producers, and document supported scope and domain boundaries.

## 4. Observation and workflow acceptance

- [x] 4.1 Extend workflow descriptions and consumer input-resolution observations with group and selected-source identities; update manifest/event fixtures and readers together, and verify diagnostics expose no payloads or artificial merge nodes.
- [x] 4.2 Use resolved skip causes across Task and operator observation paths; verify an unselected candidate is not reported as a consumer skip cause when the group resolves successfully.
- [x] 4.3 Add a runnable core-node conditional convergence example and document migration from separate branch consumers; verify both branches produce one common result in memory and through `mf compile` and `mf run`.

## 5. Integration validation

- [ ] 5.1 Run `nix develop --command python3 scripts/affected_tests.py --worktree --run` and resolve failures; preserve an empty selection without invoking an unfiltered Rust suite.
- [ ] 5.2 Run `openspec validate add-exclusive-input-bindings --strict`, `nix develop --command prek install`, and `nix develop --command prek -a`; verify all applicable checks and separately check newly created untracked documentation files.
- [ ] 5.3 Review the implementation against both spec deltas, with emphasis on ordinary-workflow compatibility, same-frame selection, generated parity, and secret handling; record any local experiments under ignored `target/`.

## Workflow follow-up

- Archive after implementation and review are complete.
- Update the separately maintained test-selection workflow plan to consume exclusive input bindings instead of introducing `builtin.merge` when that work resumes.
