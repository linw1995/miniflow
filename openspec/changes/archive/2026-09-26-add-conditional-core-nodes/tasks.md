# Tasks

## 1. Consolidate basic node packages

- [x] 1.1 Create `mfn-core` with constant and identity modules, remove the old workspace crates, and update workspace/dev dependencies and the Cargo lock; verify existing constant/identity execution and registry tests pass unchanged in behavior.
- [x] 1.2 Migrate examples, fixtures, packaged CLI tests, and release support package lists to `mfn-core`; verify explicit linkage retains both kinds and conflicting providers still fail registry validation.
- [x] 1.3 Update workflow, plugin, and release documentation with package boundaries and dependency/lock migration; verify the migrated hello workflow compiles and returns its previous output.

## 2. Plan explicit dependencies and resolve instance metadata

- [x] 2.1 Add optional `control_edges` with empty-default serialization and plan the union of data/control dependencies; verify combined cycles, duplicate controls, unknown endpoints, deduplicated node-pair indegrees, deterministic ordering, and old definition round-trips.
- [x] 2.2 Use one static/dynamic port descriptor with registration fallback, and instance context-reference declarations; verify an unchanged external plugin builds and dynamic instances expose distinct port sets and source references.
- [x] 2.3 Refactor compiler validation to construct once per pass and index effective outputs by `${node_id}.${output_name}`; verify local port/type checks, exact reference lookup, dotted names, unknown keys, and collision diagnostics naming both source pairs.
- [x] 2.4 Validate context producers as strict ancestors through explicit dependencies without adding edges; verify direct/transitive references, unknown/self/descendant/unordered sources, and rejection despite an earlier lexical tie-break.
- [x] 2.5 Carry resolved descriptors and reference declarations into prepared flow nodes; require complete port metadata when constructing execution nodes and document control edges and instance metadata in workflow/plugin guides.

## 3. Implement execution context and conditional states

- [x] 3.1 Add a fresh per-run context with flat qualified output IDs, read-only views, and atomic publication; verify single runtime prefixing, same-name outputs from different nodes, branch activation/skip identities, transitive reads, unavailable/null states, and run isolation.
- [x] 3.2 Add the default context-aware execution adapter and disjoint produced/skipped result; verify old nodes execute once, context-free router calls fail clearly, and invalid skip markers, overlaps, and plugin errors remain failures.
- [x] 3.3 Implement shared data/control dependency resolution and skip propagation with canonical order and missing-output precedence; verify produced false/null controls activate, optional inputs, mixed states, fan-out, nested skips, independent nodes, and ordinary joins.
- [x] 3.4 Integrate shared helpers and context publication into `Flow::execute` and generated direct orchestration; verify actual binaries match in-memory values, errors, traces, and context visibility without interpreting a graph at runtime.
- [x] 3.5 Document qualified output IDs, local plugin names, collision rules, context lifetime, compile-time reference declarations, and factory constraints; verify a third-party fixture reads qualified context outputs and reports skips without compiler kind-name handling.

## 4. Add optional workflow output selections

- [x] 4.1 Add `optional` with a default of false and serialization omission for false, updating Rust struct callers and compiled plan round-trips; verify old schema `2026-09-26` fixtures preserve their semantics and invalid optional values are rejected.
- [x] 4.2 Share selected-output extraction across execution paths; verify required skipped errors, optional omission, null preservation, unexpected missing errors, invalid selected ports, duplicate names, and the empty result object.
- [x] 4.3 Document optional output syntax and support-package compatibility in `docs/workflows.md`; verify the documented output examples match executable fixtures.

## 5. Add ordered if-else routing

- [x] 5.1 Implement strict branch/predicate/source parsing and dynamic outputs in `mfn-core`; verify no data inputs, at least one condition, exactly one more output than conditions, missing/empty branches, invalid IDs, unknown fields, and stable output names after reordering.
- [x] 5.2 Resolve qualified `source.output` and `source.path` through context for `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `exists`, and `not_exists`; verify pointer syntax, exact output keys, operator/literal validation, availability states, scalar rules, and qualified source/branch/path diagnostics.
- [x] 5.3 Implement numeric comparison using normalized runtime number representations; verify numeric equality across forms, adjacent integers above 2^53, signed/unsigned boundaries, decimals, exponent forms, and no numeric-string coercion.
- [x] 5.4 Implement ordered short-circuit evaluation through context-aware execution; verify first match wins, else-if, fallback, reached errors, unreachable data errors, validation of every static reference, selected true activation, and unchanged source outputs.
- [x] 5.5 Add runnable one-condition and multi-condition examples where the immediate control predecessor differs from the condition's source node; verify explicit ancestor paths, predicates reading different producers, optional branch results, and downstream business data bindings.
- [x] 5.6 Extend actual-binary acceptance fixtures with inactive side-effect/failure nodes, nested branches, multiple instances, and branch-order edits in a reused build directory; verify predicate results, selected execution only, and matching in-memory traces.

## 6. Validate integration and release readiness

- [x] 6.1 Verify packaged core nodes and matching support crates work with the installed CLI outside the checkout, and the resulting conditional binary runs without build inputs, using the packaged acceptance harness.
- [x] 6.2 In `nix develop`, run `prek install`, `prek -a`, and `nix flake check -L`; resolve failures and record the final check results.
- [x] 6.3 Run `nix develop --command bash scripts/run-cov.sh` and inspect coverage for selection, skip propagation, missing-output precedence, and optional results; add meaningful cases for uncovered control-flow behavior.
- [x] 6.4 Run `openspec validate add-conditional-core-nodes --strict` and review every scenario against the delivered tests before marking the implementation complete.

## 7. Ablate unnecessary machinery

- [x] 7.1 Establish a common behavior baseline and test removal of numeric precision handling and ancestor checks; retain both after reproducible regressions.
- [x] 7.2 Remove obsolete runner helpers, the flow error wrapper, optional execution metadata, and duplicate port descriptors; verify the common behavior suite after each change.
- [x] 7.3 Replace runtime registration, reverse indexing, and read whitelists with a flat completed-output context; retain compiler ordering checks and workflow error behavior.
- [x] 7.4 Merge overlapping generated-binary tests and remove tests coupled only to deleted low-level APIs; verify retained fault detection through a controlled code-generation mutation.
- [x] 7.5 Run full workspace and Nix checks.
