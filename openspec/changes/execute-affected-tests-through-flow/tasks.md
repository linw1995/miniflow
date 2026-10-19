## 1. Preserve the consumer boundary

- [ ] 1.1 Add stdin plan input and separate machine reports from child diagnostics, preserving path callers and failure/empty behavior.
- [ ] 1.2 Extend existing consumer verification and commit this group.

## 2. Add the missing process capability

- [ ] 2.1 Implement `process.run` with typed errors, separate argv, JSON stdin, bounded output, timeout/cancellation, and owned group cleanup.
- [ ] 2.2 Verify success, failure, malformed output, bounds, sensitivity, and cancellation; commit this group.

## 3. Compose optional local execution

- [ ] 3.1 Carry opaque execution context through Git/LSP/planning; add the default-false run parameter and existing conditional routing.
- [ ] 3.2 Preserve plan-only output, reject CI execution, and expose optional execution reports through mf.
- [ ] 3.3 Extend existing Flow replay and invocation documentation, rebuild the local runner, and commit this group.

## 4. Review and archive

- [ ] 4.1 Run focused tests and required hooks; ablate gating/report separation and remove unnecessary abstractions or tests.
- [ ] 4.2 Review AGENTS.md compliance, synchronize formal specs, and archive in a separate commit.
