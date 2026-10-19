## 1. Preserve the consumer boundary

- [x] 1.1 Add stdin plan input and separate machine reports from child diagnostics, preserving path callers and failure/empty behavior.
- [x] 1.2 Extend existing consumer verification and commit this group.

## 2. Add the missing process capability

- [x] 2.1 Implement `process.run` with typed errors, separate argv, JSON stdin, bounded output, timeout/cancellation, and owned group cleanup.
- [x] 2.2 Verify success, failure, malformed output, bounds, sensitivity, and cancellation; commit this group.

## 3. Compose optional local execution

- [x] 3.1 Carry opaque execution context through Git/LSP/planning; add the default-false run parameter and existing conditional routing.
- [x] 3.2 Preserve plan-only output, reject CI execution, and expose optional execution reports through mf.
- [x] 3.3 Extend existing Flow replay and invocation documentation, rebuild the local runner, and commit this group.

## 4. Review and archive

- [x] 4.0 Bridge standalone runner interrupts to existing cancellation and verify native child cleanup before archival.

- [ ] 4.1 Run focused tests and required hooks; ablate gating/report separation and remove unnecessary abstractions or tests.
- [ ] 4.2 Review AGENTS.md compliance, synchronize formal specs, and archive in a separate commit.
