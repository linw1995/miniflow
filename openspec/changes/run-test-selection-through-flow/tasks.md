## 1. Integrate upstream and prove existing-node composition

- [x] 1.1 Integrate origin/main through #165 and #161 without rewriting published history; preserve the current PR's tooling and CI changes.
- [x] 1.2 Build a minimal single-definition fixture Flow using parameters, if-else, Code, Loop/state, and exclusive inputs; verify both local and CI paths through `mf`.
- [x] 1.3 Prove compatibility predicates, ancestor ordering/ties, empty-candidate handling, early stopping, cumulative fallback, cancellation, and fatal current-identity handling using actual supported contracts.
- [x] 1.4 Specify concrete external I/O ports, availability/error semantics, elapsed/item bounds, and the smallest required kind inventory; resolve any discovered reusable-contract gaps in the spec before production implementation.
- [x] 1.5 Record design-gate evidence under target/ and commit upstream integration and the verified design separately.

## 2. Implement only the missing external capabilities

- [ ] 2.1 Reuse or add the GitHub metadata/artifact read operations identified by the gate, with bounded decoding, provenance, cancellation, and isolated credentials.
- [ ] 2.2 Reuse existing Git helpers and add only the missing revision facts or explicit object-retrieval operations, preserving caller state and offline defaults.
- [ ] 2.3 Verify current-fatal versus historical-unavailable behavior and ensure no provider contains baseline eligibility, candidate-search, or fallback policy; commit each cohesive capability group separately.

## 3. Complete the common comparison and planning contract

- [ ] 3.1 Extend `git.diff` with normalized requests, immutable worktree snapshots, first-parent/root-commit comparison, and explicit-ref compatibility.
- [ ] 3.2 Support package-only CI selection and optional local semantic refinement through the existing LSP/planner path.
- [ ] 3.3 Carry comparison/validation provenance through the producer boundary and compare package selection with the current selector; commit this group.

## 4. Compose the production Flow

- [ ] 4.1 Build both branches in the existing single Flow definition, using Code for policy and bounded Loop/if-else for lazy candidate search.
- [ ] 4.2 Converge complete comparison requests through proven exclusive edges into the shared Git/LSP/planner path.
- [ ] 4.3 Verify the same compiled executable works locally without CI setup and in CI with exactly one explicit plan; validate early-stop I/O counts and failure propagation.
- [ ] 4.4 Update invocation/configuration documentation and commit the Flow integration.

## 5. Migrate CI and remove duplicated ownership

- [ ] 5.1 Invoke the compiled Flow through `mf` for CI selection; preserve summaries, outputs, and comparison/context artifacts using thin adapters.
- [ ] 5.2 Define migration compatibility/policy version and verify the required first-run cumulative reset and later incremental reuse.
- [ ] 5.3 Update AGENTS.md and local entry points; remove duplicate Python selection/discovery logic and superseded tests only after parity evidence passes.
- [ ] 5.4 Preserve package-only coverage, empty plans, failed-gate behavior, and final attempt-bound success publication; commit this group.

## 6. Review, ablate, and archive

- [ ] 6.1 Run pinned-toolchain checks and required hooks; compare planning outputs and phase-specific costs on recorded local/CI cases.
- [ ] 6.2 Ablate policy predicates, candidate order, early stopping, error propagation, and local isolation; remove unnecessary graph steps/abstractions/tests and keep reports under target/.
- [ ] 6.3 Review repository rules, synchronize formal specs and the obsolete three-node purpose text, and archive the completed change in a separate commit.
