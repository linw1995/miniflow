## 1. Simplify snapshot notifications and consumers

- [x] 1.1 Remove snapshot stream identities, run binding, and unused observation accessors.
- [x] 1.2 Simplify Data payloads and adapt telemetry, runner, TUI, and existing test consumers.
- [x] 1.3 Remove emission identity bookkeeping and only notify successfully admitted producer outputs; validate and commit this group.

## 2. Verify uniform capture behavior

- [x] 2.1 Strengthen existing cases for same-span equal emissions, distinct recording sequences, native correlation, and nested scopes.
- [x] 2.2 Inspect raw snapshots without observation in the producer failure case and ablate the admission guard.
- [x] 2.3 Update documentation and remove unnecessary design or duplicated assertions; commit this group.

## 3. Review and complete

- [x] 3.1 Review AGENTS.md compliance and local ablation results.
- [x] 3.2 Run affected coverage and prek -a with the pinned toolchain under origin/main AGENTS.md; commit final corrections.
- [x] 3.3 Archive the reviewed specification, update PR 142, and monitor final CI and review feedback.
