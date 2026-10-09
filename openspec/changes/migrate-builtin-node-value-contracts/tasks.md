## 1. Fixed builtin contracts

- [ ] 1.1 Migrate Constant to `NodeValue` and `TypedTaskNode`, preserving literal evidence and output refinements.
- [ ] 1.2 Migrate Batch and Readline fixed bags to `NodeValue`, preserving broad metadata, timing, ownership, and error sources.
- [ ] 1.3 Review configured-port nodes and document their dynamic boundary without adding artificial schemas.

## 2. Builtin generation and completion

- [ ] 2.1 Advertise certified Identity generation through an exported constructor and one shared typed handle.
- [ ] 2.2 Extend existing behavioral regressions with builtin generation, refined fallback, invalid inputs, and payload identity.
- [ ] 2.3 Review AGENTS.md compliance, ablate unnecessary code/tests, run pinned hooks, complete Nix checks, focused coverage, and strict change validation.
- [ ] 2.4 Commit each implementation group, archive the reviewed change, validate main specifications, and update the current PR.
