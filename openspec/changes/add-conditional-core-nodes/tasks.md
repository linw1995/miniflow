# Tasks

## 1. Consolidate basic node packages

- [ ] 1.1 Create `mfn-core` with constant and identity modules, remove the old workspace crates, and update workspace/dev dependencies and the Cargo lock; verify existing constant/identity execution and registry tests pass unchanged in behavior.
- [ ] 1.2 Migrate examples, fixtures, packaged CLI tests, and release support package lists to `mfn-core`; verify explicit linkage retains both kinds and conflicting providers still fail registry validation.
- [ ] 1.3 Update workflow, plugin, and release documentation with package boundaries and dependency/lock migration; verify the migrated hello workflow compiles and returns its previous output.

## 2. Plan explicit dependencies and resolve instance metadata

- [ ] 2.1 Add optional `control_edges` with empty-default serialization and plan the union of data/control dependencies; verify combined cycles, duplicate controls, unknown endpoints, deduplicated node-pair indegrees, deterministic ordering, and old definition round-trips.
- [ ] 2.2 Add owned effective port descriptors, static registration fallback, and instance context-reference declarations; verify an unchanged external plugin builds and dynamic instances expose distinct port sets and source references.
- [ ] 2.3 Refactor compiler validation to construct once per pass and index effective outputs by `${node_id}.${output_name}`; verify local port/type checks, exact reference lookup, dotted names, unknown keys, and collision diagnostics naming both source pairs.
- [ ] 2.4 Validate context producers as strict ancestors through explicit dependencies without adding edges; verify direct/transitive references, unknown/self/descendant/unordered sources, and rejection despite an earlier lexical tie-break.
- [ ] 2.5 Carry resolved descriptors and reference declarations into prepared flow nodes; verify lower-level callers receive contextual metadata errors and document control edges and instance metadata in workflow/plugin guides.

## 3. Implement execution context and conditional states

- [ ] 3.1 Add a fresh per-run context with flat qualified output IDs, declared read-only views, and atomic publication; verify single runtime prefixing, same-name outputs from different nodes, branch activation/skip identities, transitive reads, pending/missing/null states, and run isolation.
- [ ] 3.2 Add the default context-aware execution adapter and disjoint produced/skipped result; verify old nodes execute once, context-free router calls fail clearly, and invalid skip markers, overlaps, and plugin errors remain failures.
- [ ] 3.3 Implement shared data/control dependency resolution and skip propagation with canonical order and missing-output precedence; verify produced false/null controls activate, optional inputs, mixed states, fan-out, nested skips, independent nodes, and ordinary joins.
- [ ] 3.4 Integrate shared helpers and context publication into `Flow::execute` and generated direct orchestration; verify actual binaries match in-memory values, errors, traces, and context visibility without interpreting a graph at runtime.
- [ ] 3.5 Document qualified output IDs, local plugin names, collision rules, context lifetime, reference declarations, and factory constraints; verify a third-party fixture reads qualified context outputs and reports skips without compiler kind-name handling.

## 4. Add optional workflow output selections

- [ ] 4.1 Add `optional` with a default of false and serialization omission for false, updating Rust struct callers and compiled plan round-trips; verify old schema `2026-09-26` fixtures preserve their semantics and invalid optional values are rejected.
- [ ] 4.2 Share selected-output extraction across execution paths; verify required skipped errors, optional omission, null preservation, unexpected missing errors, invalid selected ports, duplicate names, and the empty result object.
- [ ] 4.3 Document optional output syntax and support-package compatibility in `docs/workflows.md`; verify the documented output examples match executable fixtures.

## 5. Add ordered if-else routing

- [ ] 5.1 Implement strict branch/predicate/source parsing and dynamic outputs in `mfn-core`; verify no data inputs, at least one condition, exactly one more output than conditions, missing/empty branches, invalid IDs, unknown fields, and stable output names after reordering.
- [ ] 5.2 Resolve qualified `source.output` and `source.path` through context for `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `exists`, and `not_exists`; verify pointer syntax, exact output keys, operator/literal validation, availability states, scalar rules, and qualified source/branch/path diagnostics.
- [ ] 5.3 Implement numeric comparison using normalized runtime number representations; verify numeric equality across forms, adjacent integers above 2^53, signed/unsigned boundaries, decimals, exponent forms, and no numeric-string coercion.
- [ ] 5.4 Implement ordered short-circuit evaluation through context-aware execution; verify first match wins, else-if, fallback, reached errors, unreachable data errors, validation of every static reference, selected true activation, and unchanged source outputs.
- [ ] 5.5 Add runnable one-condition and multi-condition examples where the immediate control predecessor differs from the condition's source node; verify explicit ancestor paths, predicates reading different producers, optional branch results, and downstream business data bindings.
- [ ] 5.6 Extend actual-binary acceptance fixtures with inactive side-effect/failure nodes, nested branches, multiple instances, and branch-order edits in a reused build directory; verify predicate results, selected execution only, and matching in-memory traces.

## 6. Validate integration and release readiness

- [ ] 6.1 Verify packaged core nodes and matching support crates work with the installed CLI outside the checkout, and the resulting conditional binary runs without build inputs, using the packaged acceptance harness.
- [ ] 6.2 In `nix develop`, run `prek install`, `prek -a`, and `nix flake check -L`; resolve failures and record the final check results.
- [ ] 6.3 Run `nix develop --command bash scripts/run-cov.sh` and inspect coverage for selection, skip propagation, missing-output precedence, and optional results; add meaningful cases for uncovered control-flow behavior.
- [ ] 6.4 Run `openspec validate add-conditional-core-nodes --strict` and review every scenario against the delivered tests before marking the implementation complete.
