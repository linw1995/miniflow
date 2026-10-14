## 1. Separate policy from execution

- [ ] 1.1 Replace the runtime recorder with current-value notifications and preserve publication identities outside scheduler locks.
- [ ] 1.2 Move capture encoding/state into telemetry and wire runner-owned capture completion.
- [ ] 1.3 Adapt existing integration coverage and verify raw notifications preserve values above telemetry budgets.

## 2. Review and submit

- [ ] 2.1 Review module boundaries, typed errors, and ablation results against AGENTS.md.
- [ ] 2.2 Run affected coverage, prek, and the pinned Nix flake checks; commit the implementation.
- [ ] 2.3 Archive the reviewed spec, push the PR updates, and monitor checks and reviews.
