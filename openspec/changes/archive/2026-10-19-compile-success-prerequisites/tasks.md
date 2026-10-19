## 1. Compile activation conditions

- [x] 1.1 Separate compiler success prerequisites from runtime data dependencies and derive normalized activation clauses.
- [x] 1.2 Bind in-memory task/body/stream predicates and emit static generated guards, preserving startup interfaces and typed transfers.

## 2. Remove runtime success interpretation

- [x] 2.1 Remove NodeSuccess dependency dispatch, task outcome storage, frame copying and missing-outcome errors.
- [x] 2.2 Reuse input validation, cancellation, publication and observation lifecycle with prepared skip causes.

## 3. Validate and publish

- [x] 3.1 Replace outcome-implementation tests with activation and generated execution regressions; run ablations.
- [x] 3.2 Fix the Nix fixture omission, run repository checks and affected suites, and review the final diff.
- [x] 3.3 Update documentation, review the specification for archival, and prepare the PR update.
- [x] 3.4 Ablate duplicate Flow/Stream dispatch, redundant forwarding and guard-domain checks, and unshared predicate ownership; retain existing behavioral coverage.
